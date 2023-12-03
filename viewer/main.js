// Copyright © 2023 David Caldwell <david@porkrind.org>

import { URL } from 'node:url';
import { createRequire } from 'node:module';
import path from 'node:path';
import fs from 'node:fs';

// CommonJS backwards compat
const __dirname = path.dirname(new URL(import.meta.url).pathname);
const require = createRequire(import.meta.url);
const { app, BrowserWindow, Menu, MenuItem, ipcMain, dialog } = require('electron');
const pdpfs = require(__dirname);

class Image {
    static images = {};

    path;
    id;

    constructor({image_path,image_id}) {
        this.path = image_path;
        if (image_id != undefined)
            this.id = image_id;
        else if (image_path != undefined)
            this.id = pdpfs.open_image(image_path);
        else throw("Need image_path or image_id");
        Image.images[this.id] = this;
    }

    static create(image_type, device_type, image_size, filesystem) {
        let image_id = pdpfs.create_image(image_type, device_type, image_size, filesystem);
        return new Image({image_id});
    }

    get_directory_entries()          { return pdpfs.get_directory_entries(this.id)            }
    cp_into_image        (path)      { return pdpfs.cp_into_image        (this.id, path)      }
    cp_from_image        (src, dest) { return pdpfs.cp_from_image        (this.id, src, dest) }
    image_is_dirty       ()          { return pdpfs.image_is_dirty       (this.id)            }
    mv                   (src, dest) { return pdpfs.mv                   (this.id, src, dest, false) }
    rm                   (path)      { return pdpfs.rm                   (this.id, path)      }
    save                 ()          { return pdpfs.save                 (this.id, this.path) }
    convert              (file,
                          image_type){ return pdpfs.convert              (this.id, file, image_type) }

    close() {
        pdpfs.close_image(this.id);
        delete Image.images[this.id];
    }
}

class ImageWindow {
    static windows = {};

    static from_id(id) {
        return this.windows[id]
    }

    id;
    window;
    image;

    constructor(image) {
        this.image = image;
        this.selected = [];
        if (image) {
            this.create_temp_path();
            this.create_window();
        } else
            this.create_new_window();
    }

    create_window() {
        let win = new BrowserWindow({
            width: 800,
            height: 600,
            webPreferences: {
                preload: path.join(__dirname, 'preload.js')
            },
            title: this.title(),
        })
        this.window = win;
        ImageWindow.windows[win.id] = this;

        this.setup_titlebar();

        win.loadFile('web/index.html', { query: { id: this.image.id } })

        const modifies = (f) =>
              (...args) => {
                  let ret = f(...args);
                  this.update_edited();
                  return ret;
              };

        win.on('close', (event) => this.close(event));
        win.on('closed', (event) => this.closed(event))
        win.on('focus', (event) => this.focus(event));
        win.webContents.ipc.on('app:set_selected', (event, selected) => this.set_selected(selected));
        win.webContents.ipc.on('ondragstart', (event) => this.drag_start());
        win.webContents.ipc.handle('pdpfs:rm', (event, ...files) => this.rm(...files));
        win.webContents.ipc.handle('pdpfs:mv', (event, src, dest) => this.mv(src, dest));
        win.webContents.ipc.handle('pdpfs:get_directory_entries',          (event)       => this.image.get_directory_entries());
        win.webContents.ipc.handle('pdpfs:image_is_dirty',                 (event)       => this.image.image_is_dirty());
        win.webContents.ipc.handle('pdpfs:cp_into_image',         modifies((event, path) => this.image.cp_into_image(path)));
        win.webContents.ipc.handle('pdpfs:save',                  modifies((event)       => this.image.save()));
        win.on('menu:file/save', async (event) => await this.save());
        win.on('menu:file/save-as', async (event) => await this.save_as());
        win.on('menu:file/delete', (event) => this.rm_selected());
    }

    send(type, detail) {
        this.window.webContents.send('pdpfs', type, detail)
    };

    async close(event) {
        if (!await this.image.image_is_dirty())
            return;
        event.preventDefault();

        let { response } = await dialog.showMessageBox(this.window, {
            message: this.image.path ? `${path.basename(this.image.path)} has unsaved changes.`
                                     : 'This disk image has not been saved.',
            type: "question",
            buttons: this.image.path ? ["&Cancel", "&Discard Changes", "&Save Changes"]
                                     : ["&Cancel", "&Discard",         "&Save"],
            defaultId: 2,
            normalizeAccessKeys: true,
        });

        // We already prevented default so Cancel is handled.
        if (response == 1) // Discard
            this.window.destroy();
        if (response == 2) { // Save
            this.save();
            this.window.close();
        }
    }

    closed(event) {
        if (this.temp_path)
            this.clean_temp_path();
        if (this.image)
            this.image.close();
        delete ImageWindow.windows[this.window.id];
    }

    title() {
        return `${pdpfs.filesystem_name(this.image.id)}: ${this.image.path ?? "(Unsaved)"}`
    }

    setup_titlebar() {
        if (this.image.path)
            this.window.setRepresentedFilename(this.image.path);
        this.window.setTitle(this.title());
    }

    focus(event) {
        update_menus(this.selected, true)
    }

    set_selected(selected) {
        update_menus(this.selected = selected, true);
    }

    create_temp_path() {
        // Sadly because of the way Electron drag and drop works, we _have_ to have a file on the disk as the source when we drag.
        this.paths_to_clean ??= {};
        this.temp_path = fs.mkdtempSync(path.join(app.getPath("temp"), "image-XXXXXXXX"));
    }

    clean_temp_path() {
        try {
            for (let f of Object.keys(this.paths_to_clean))
                fs.rmSync(path.join(this.temp_path, f), { force: true });
            fs.rmdirSync(this.temp_path);
        } catch(e) {
            console.log(`Got error cleaning up ${this.temp_path}:`, e); // Temp files: don't bug the user--logging is good enough.
        }
    }

    drag_start() {
        let filenames = this.selected;
        console.log(`dragging [${this.image.id}] ${this.temp_path}/{${filenames.join(',')}}...`);

        for (let f of filenames) {
            this.image.cp_from_image(f, path.join(this.temp_path, f));
            this.paths_to_clean[f] = true;
        }

        this.window.webContents.startDrag({
            files: filenames.map(f => path.join(this.temp_path, f)),
            icon: path.join(__dirname, filenames.length == 1 ? 'web/stack-96.png' : 'web/stack-96.png'),
        })
    }

    async update_edited() {
        this.window.setDocumentEdited(await this.image.image_is_dirty());
    }

    async update_entries() {
        this.send('pdpfs:refresh-directory-entries', { entries: this.image.get_directory_entries() });
    }

    mv(src, dest) {
        this.image.mv(src, dest);
        this.update_edited();
        this.update_entries();
    }

    rm_selected() {
        this.rm(...this.selected)
    }

    rm(...files) {
        for (let file of files)
            this.image.rm(file);
        this.update_entries();
        this.update_edited();
    }

    async save() {
        if (!this.image.path)
            return this.save_as({save: true});
        try {
            this.image.save();
            this.update_edited();
        } catch(e) {
            await dialog.showErrorBox(`Could not save ${path.basename(this,image.path)}:`, e.toString());
            return;
        }
    }

    async save_as({save}={}) {
        let { canceled, filePath, bookmark } = await dialog.showSaveDialog(this.window, {
            title: `Save this disk image${!save ? ' as' : ''}:`,
            defaultPath: "Disk Image.img",
            ...(save ? {} : {
                filters: [ { name: "IMG", extensions: [".img"] },
                           { name: "IMD", extensions: [".imd"] },],
            }),
            message: "Above the text fields we dream",
            properties: ['createDirectory', 'showOverwriteConfirmation'],
        });
        if (canceled) return;
        try {
            if (!save) {
                await this.image.convert(filePath, path.extname(filePath).slice(1));
                let converted = new Image({image_path: filePath});
                this.image.close();
                this.image = converted;
            } else {
                this.image.path = filePath;
                this.image.save();
            }
        } catch(e) {
            await dialog.showErrorBox(`Could not save ${path.basename(filePath)}:`, e.toString());
            return;
        }
        this.setup_titlebar();
        this.update_edited();
    }
}

class NewImageWindow {
    constructor() {
        let win = new BrowserWindow({
            width: 400,
            height: 300,
            resizable: false,
            minimizable: false,
            maximizable: false,
            closable: false,
            webPreferences: {
                preload: path.join(__dirname, 'preload.js'),
                additionalArguments: [JSON.stringify({ device_types: pdpfs.device_types(),
                                                       image_types:  pdpfs.image_types(),
                                                       filesystems:  pdpfs.filesystems() })],
            },
            title: "New Image",
        })
        this.window = win;
        ImageWindow.windows[win.id] = this;

        win.loadFile('web/index.html', { query: { kind: 'new' } })

        win.on('closed', (event) => this.closed(event))
        win.on('focus', (event) => this.focus(event));
        win.webContents.ipc.on('new:cancel', () => this.cancel())
        win.webContents.ipc.on('new:create', (event, image_type, device_type, image_size, filesystem) => this.create(image_type, device_type, image_size, filesystem))
    }

    closed() {
        delete ImageWindow.windows[this.window.id];
    }

    focus(event) {
        update_menus([], false)
    }

    cancel() {
        this.window.destroy();
    }

    create(image_type, device_type, image_size, filesystem) {
        let image = Image.create(image_type, device_type, image_size, filesystem);
        new ImageWindow(image);
        this.window.destroy();
    }
}

async function open_image_dialog() {
    console.log("open_image_dialog()");
    const { canceled, filePaths } = await dialog.showOpenDialog();
    if (canceled) return undefined;

    console.log("file_paths", filePaths);
    return open_image(filePaths[0]);
}

function open_image(image_path) {
    try {
        new ImageWindow(new Image({image_path}));
    } catch(e) {
        dialog.showErrorBox(`Unable to open ${path.basename(image_path)}`,
                            `There was an error loading the image: ${e}`);
    }
}

const update_menus = (selected, is_image_window) => {
    enable_menu_items("sel", selected.length > 0);
    enable_menu_items("one_sel", selected.length == 1);
    enable_menu_items("img", is_image_window);
}

app.on('open-file', (event, path) => {
    event.preventDefault();
    open_image(path);
})

app.on('menu:file/new', (event) => {
    new NewImageWindow()
});
app.on('menu:file/open', (event) => {
    open_image_dialog();
})

const shortcut = (key)      => process.platform == 'darwin' ? `Cmd+${key}` : `Ctrl+${key}`;
const mac      = (...items) => process.platform == 'darwin' ? items : [];
const non_mac  = (...items) => process.platform != 'darwin' ? items : [];
const menu_emit = (menu_item) => {
    let event = `menu:${menu_item.id}`;
    let win = BrowserWindow.getFocusedWindow();
    if (win?.emit(event, menu_item) == true)
        return;
    if (app.emit(event, menu_item) == true)
        return;
    if (win)
        ImageWindow.from_id(win.id)?.send(event);
}

const __need = {};
const extract_needs = (template) => {
    let id = 0;
    return template.map((toplevel) => {
        if (toplevel.submenu != undefined)
            toplevel.submenu = toplevel.submenu.map((m) => {
                if (m.need) {
                    if (!m.id)
                        m.id = `need_${m.need.join('_')}:${id++}`;
                    for (let n of m.need) {
                        __need[n] ??= [];
                        __need[n].push(m.id);
                    }
                }
                return m;
            });
        return toplevel;
    });
}

const enable_menu_items = (need, enable) => {
    for (let id of __need[need]) {
        let menu = Menu.getApplicationMenu().getMenuItemById(id);
        menu.enabled = enable;
    }
}

const menu = new Menu.buildFromTemplate(
    extract_needs([
        ...mac({ role: 'appMenu' }),
        { label: 'File',
          submenu: [
              { beforeGroupContaining: ['Quit'],
                label: 'New Disk Image…',     id: 'file/new',     click: menu_emit,               accelerator: shortcut('N') },
              { label: 'Open Disk Image…',    id: 'file/open',    click: menu_emit,               accelerator: shortcut('O') },
              { role: 'recentDocuments' },
              { type: 'separator' },
              { role: 'close',                id: 'file/close',   click: menu_emit },
              { label: 'Save Disk Image',     id: 'file/save',    click: menu_emit, need:["img"], accelerator: shortcut('S') },
              { label: 'Save Disk Image As…', id: 'file/save-as', click: menu_emit, need:["img"] },
              { type: 'separator' },
              { label: 'Export Files…',       id: 'file/export',  click: menu_emit, need:["sel"] },
              { label: 'Import Files…',       id: 'file/import',  click: menu_emit, need:["img"] },
              { label: 'Delete',              id: 'file/delete',  click: menu_emit, need:["sel"], accelerator: shortcut('Backspace')},
              { label: 'Rename',              id: 'file/rename',  click: menu_emit, need:["one_sel"] },
              ...non_mac({ type: 'separator' },
                         { role: 'quit' }),
          ],
        },
        { role: 'editMenu' },
        { role: 'viewMenu' },
        { role: 'windowMenu' },
        { role: 'help',
          submenu: [
              { label: 'There is no help for you' }
          ]
        },
    ])
);
Menu.setApplicationMenu(menu);

app.whenReady().then(() => {
    open_image_dialog();
})

app.on('window-all-closed', () => {
    // if (process.platform !== 'darwin')
      app.quit()
})

app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) open_image_dialog();
})


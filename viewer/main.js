// Copyright © 2023 David Caldwell <david@porkrind.org>

const { app, BrowserWindow, Menu, MenuItem, ipcMain, dialog } = require('electron');
const path = require('node:path');
const fs = require('node:fs');
const pdpfs = require(__dirname);

class Image {
    static images = {};

    path;
    id;

    constructor(image_path) {
        this.path = image_path;
        this.id = pdpfs.open_image(image_path);
        Image.images[this.id] = this;
    }

    get_directory_entries()          { return pdpfs.get_directory_entries(this.id)            }
    cp_into_image        (path)      { return pdpfs.cp_into_image        (this.id, path)      }
    image_is_dirty       ()          { return pdpfs.image_is_dirty       (this.id)            }
    mv                   (src, dest) { return pdpfs.mv                   (this.id, src, dest, false) }
    rm                   (path)      { return pdpfs.rm                   (this.id, path)      }
    save                 ()          { return pdpfs.save                 (this.id, this.path) }
    extract_to_path      (path)      { return pdpfs.extract_to_path      (this.id, path)      }
    convert              (file,
                          image_type){ return pdpfs.convert              (this.id, file, image_type) }


    close() {
        delete Image.images[this.id];
    }
}

class ImageWindow {
    static windows = {};

    static from_id(id) {
        return this.windows[id]
    }

    path;
    id;
    window;
    image;

    constructor(image) {
        this.image = image;
        this.selected = [];
        this.create_temp_path();
        this.create_window(`${pdpfs.filesystem_name(this.image.id)}: ${this.image.path}`);
    }

    create_window(title) {
        let win = new BrowserWindow({
            width: 800,
            height: 600,
            webPreferences: {
                preload: path.join(__dirname, 'preload.js')
            },
            title: title,
        })
        this.window = win;
        ImageWindow.windows[win.id] = this;

        win.setRepresentedFilename(this.image.path);

        win.loadFile('web/index.html', { query: { id: this.image.id } })

        win.on('close', (event) => this.close(event));
        win.on('closed', (event) => this.closed(event))
        win.on('focus', (event) => this.focus(event));
    }

    send(type, detail) {
        this.window.webContents.send('pdpfs', type, detail)
    };

    async close(event) {
        if (!await pdpfs.image_is_dirty(this.image.id))
            return;
        event.preventDefault();

        let { response } = await dialog.showMessageBox(this.window, {
            message: `${path.basename(this.image.path)} has unsaved changes.`,
            type: "question",
            buttons: ["&Cancel", "&Discard Changes", "&Save Changes"],
            defaultId: 2,
            normalizeAccessKeys: true,
        });

        // We already prevented default so Cancel is handled.
        if (response == 1) // Discard
            this.window.destroy();
        if (response == 2) { // Save
            this.image.save();
            this.window.close();
        }
    }

    closed(event) {
        if (this.temp_path) {
            // cleanup
        }
        this.image.close();
        delete ImageWindow.windows[this.window.id];
    }

    focus(event) {
        update_menus(this.selected)
    }

    create_temp_path() {
        // Sadly because of the way Electron drag and drop works, we _have_ to have the file ready to go
        this.temp_path = fs.mkdtempSync(path.join(app.getPath("temp"), "image-XXXXXXXX"));
        this.image.extract_to_path(this.temp_path);
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

    async rm_selected() {
        await this.rm(...this.selected)
    }

    async rm(...files) {
        for (let file of files)
            await this.image.rm(file);
        this.update_entries();
        this.update_edited();
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
        new ImageWindow(new Image(image_path));
    } catch(e) {
        dialog.showErrorBox(`Unable to open ${path.basename(image_path)}`,
                            `There was an error loading the image: ${e}`);
    }
}

const with_image = (func) =>
      async (event, ...args) => {
          let win = BrowserWindow.fromWebContents(event.sender);
          let w = ImageWindow.from_id(win.id);
          return await func(w.image, args, w, event) //data.image.id, args, data, event);
      };

for (let api of ['get_directory_entries', 'cp_into_image', 'image_is_dirty', 'save',]) {
    ipcMain.handle(`pdpfs:${api}`, with_image(async (image, args, w) => {
        let ret = image[api](...args);
        w.update_edited();
        return ret;
    }));
}

ipcMain.handle('pdpfs:rm', with_image((image, files, w) => w.rm(...files)))
ipcMain.handle('pdpfs:mv', with_image((image, [src, dest], w) => w.mv(src, dest)))

ipcMain.on('ondragstart', with_image((image, [filenames], w) => {
    if (!filenames) filenames = w.selected;
    w.window.webContents.startDrag({
        files: filenames.map(f => path.join(w.temp_path, f)),
        icon: path.join(__dirname, filenames.length == 1 ? 'web/stack-96.png' : 'web/stack-96.png'),
    })
}))

ipcMain.on('app:set_selected', with_image((image, [selected], w) => {
    update_menus(w.selected = selected);
}))

const update_menus = (selected) => {
    enable_menu_items("sel", selected.length > 0);
    enable_menu_items("one_sel", selected.length == 1);
}

const curr_win = () => BrowserWindow.getFocusedWindow();
const curr_window = () => {
    const win_id = curr_win()?.id;
    return win_id == undefined ? undefined : ImageWindow.from_id(win_id)
}
const with_curr_window = (func) => {
    let w = curr_window();
    if (w) func(w);
}

app.on('open-file', (event, path) => {
    event.preventDefault();
    open_image(path);
})

app.on('menu:file/open', (event) => {
    open_image_dialog();
})

app.on('menu:file/save', async (event) => {
    with_curr_window(async (w) => {
        await w.image.save();
        w.update_edited();
    })
})

app.on('menu:file/save-as', async (event) => {
    with_curr_window(async (w) => {
        let { canceled, filePath, bookmark } = await dialog.showSaveDialog(w.window, {
            title: "Save this disk image as:",
            defaultPath: "Disk Image.img",
            filters: [ { name: "IMG", extensions: [".img"] },
                       { name: "IMD", extensions: [".imd"] },],
            message: "Above the text fields we dream",
            properties: ['createDirectory', 'showOverwriteConfirmation'],
        });
        if (canceled) return;
        try {
            await w.image.convert(filePath, path.extname(filePath).slice(1));
        } catch(e) {
            await dialog.showErrorBox(`Could not save ${path.basename(filePath)}:`, e.toString());
            return;
        }
        console.log(canceled, filePath, bookmark);
        w.update_edited();
    })
})

app.on('menu:file/delete', async (event) => {
    with_curr_window(async (w) => {
        w.rm_selected()
    })
})

app.on('menu:file/rename', async (event) => {
    with_curr_window(async (w) => {
        if (w.selected.length != 1) return; // Error?
        w.send('pdpfs:rename', w.selected[0]);
    })
})

const shortcut = (key)      => process.platform == 'darwin' ? `Cmd+${key}` : `Ctrl+${key}`;
const mac      = (...items) => process.platform == 'darwin' ? items : [];
const non_mac  = (...items) => process.platform != 'darwin' ? items : [];
const emitter  = (name) => (event) => app.emit(name, event);

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
                label: 'New Disk Image…',     click: emitter('menu:file/new'),                   accelerator: shortcut('N') },
              { label: 'Open Disk Image…',    click: emitter('menu:file/open'),                  accelerator: shortcut('O') },
              { role: 'recentDocuments' },
              { type: 'separator' },
              { role: 'close',                click: emitter('menu:file/close'),   need:["win"] },
              { label: 'Save Disk Image',     click: emitter('menu:file/save'),    need:["win"], accelerator: shortcut('S') },
              { label: 'Save Disk Image As…', click: emitter('menu:file/save-as'), need:["win"] },
              { type: 'separator' },
              { label: 'Export Files…',       click: emitter('menu:file/export'),  need:["sel"] },
              { label: 'Import Files…',       click: emitter('menu:file/import'),  need:["win"] },
              { label: 'Delete',              click: emitter('menu:file/delete'),  need:["sel"], accelerator: shortcut('Backspace')},
              { label: 'Rename',              click: emitter('menu:file/rename'),  need:["one_sel"] },
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


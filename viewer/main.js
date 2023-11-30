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
    mv                   (src, dest) { return pdpfs.mv                   (this.id, src, dest) }
    rm                   (path)      { return pdpfs.rm                   (this.id, path)      }
    save                 ()          { return pdpfs.save                 (this.id, this.path) }
    extract_to_path      (path)      { return pdpfs.extract_to_path      (this.id, path)      }

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

        win.on('closed', (event) => this.closed(event))
        win.on('focus', (event) => this.focus(event));
    }

    send(type, detail) {
        this.window.webContents.send('pdpfs', type, detail)
    };

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
}

async function open_image_dialog() {
    console.log("open_image_dialog()");
    const { canceled, filePaths } = await dialog.showOpenDialog();
    if (canceled) return undefined;

    console.log("file_paths", filePaths);
    return open_image(filePaths[0]);
}

function open_image(image_path) {
    new ImageWindow(new Image(image_path));
}

const with_image = (func) =>
      async (event, ...args) => {
          let win = BrowserWindow.fromWebContents(event.sender);
          let w = ImageWindow.from_id(win.id);
          return await func(w.image, args, w, event) //data.image.id, args, data, event);
      };

for (let api of ['get_directory_entries', 'cp_into_image', 'image_is_dirty', 'mv', 'rm', 'save',]) {
    ipcMain.handle(`pdpfs:${api}`, with_image(async (image, args, w) => {
        let ret = image[api](...args);
        w.window.setDocumentEdited(await image.image_is_dirty());
        return ret;
    }));
}

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
        window.setDocumentEdited(await w.image.image_is_dirty());
    })
})

app.on('menu:file/delete', async (event) => {
    with_curr_window(async (w) => {
        for (let file of w.selected)
            await w.image.rm(file);
        w.send('pdpfs:refresh-directory-entries', { entries: w.image.get_directory_entries() });
        window.setDocumentEdited(await w.image_is_dirty());
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
              { label: 'Delete Files',        click: emitter('menu:file/delete'),  need:["sel"] },
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


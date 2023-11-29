// Copyright © 2023 David Caldwell <david@porkrind.org>

const { app, BrowserWindow, Menu, MenuItem, ipcMain, dialog } = require('electron');
const path = require('node:path');
const fs = require('node:fs');
const pdpfs = require(__dirname);

const windows = {};
const images = {};

const create_fs_window = (title, data) => {
    const win = new BrowserWindow({
        width: 800,
        height: 600,
        webPreferences: {
            preload: path.join(__dirname, 'preload.js')
        },
        title: title,
    })

    win.setRepresentedFilename(data.image.path);

    data.window = win;
    data.win_id = win.id;
    data.send = (type, detail) => {
        win.webContents.send('pdpfs', type, detail)
    };


    windows[win.id] = data;

    win.loadFile('web/index.html', { query: { id: data.image.id } })

    win.on('closed', (event) => {
        if (data.temp_path) {
            // cleanup
        }
        delete images[data.image.id];
        delete windows[data.win_id];
    });
}

async function open_image_dialog() {
    console.log("open_image_dialog()");
    const { canceled, filePaths } = await dialog.showOpenDialog();
    if (canceled) return undefined;

    console.log("file_paths", filePaths);
    return open_image(filePaths[0]);
}

function open_image(image_path) {
    const id = pdpfs.open_image(image_path);

    let data = {
        image: { id: id,
                 path: image_path },
        selected: [],
    };
    images[id] = data;

    // Sadly because of the way Electron drag and drop works, we _have_ to have the file ready to go
    data.temp_path = fs.mkdtempSync(path.join(app.getPath("temp"), "image-XXXXXXXX"));
    pdpfs.extract_to_path(id, data.temp_path);

    create_fs_window(`${pdpfs.filesystem_name(id)}: ${image_path}`, data);
}

const pdpfs_wrapper = (func) =>
      async (event, ...args) => {
          let win = BrowserWindow.fromWebContents(event.sender);
          let data = windows[win.id];
          let ret = await func(data.image.id, args, data, event);
          win.setDocumentEdited(await pdpfs.image_is_dirty(data.image.id));
          return ret;
      };

ipcMain.handle('pdpfs:get_directory_entries', pdpfs_wrapper((id, args) => pdpfs.get_directory_entries(id, ...args)));
ipcMain.handle('pdpfs:cp_into_image',         pdpfs_wrapper((id, args) => pdpfs.cp_into_image        (id, ...args)));
ipcMain.handle('pdpfs:image_is_dirty',        pdpfs_wrapper((id, args) => pdpfs.image_is_dirty       (id, ...args)));
ipcMain.handle('pdpfs:mv',                    pdpfs_wrapper((id, args) => pdpfs.mv                   (id, ...args)));
ipcMain.handle('pdpfs:rm',                    pdpfs_wrapper((id, args) => pdpfs.rm                   (id, ...args)));
ipcMain.handle('pdpfs:save',                  pdpfs_wrapper((id, args) => pdpfs.save                 (id, ...args)));

ipcMain.on('ondragstart', pdpfs_wrapper((image_id, [filenames], data, event) => {
    console.log(`dragging [${image_id}] ${data.temp_path}/{${filenames.join(',')}}...`);
    event.sender.startDrag({
        files: filenames.map(f => path.join(data.temp_path, f)),
        icon: path.join(__dirname, filenames.length == 1 ? 'web/stack-96.png' : 'web/stack-96.png'),
    })
}))

ipcMain.on('app:set_selected', pdpfs_wrapper((image_id, [selected]) => {
    update_menus(images[image_id].selected = selected);
}))

const update_menus = (selected) => {
    enable_menu_items("sel", selected.length > 0);
    enable_menu_items("one_sel", selected.length == 1);
}

const curr_win = () => BrowserWindow.getFocusedWindow();
const curr_win_data = () => {
    const win_id = curr_win()?.id;
    return win_id == undefined ? undefined : windows[win_id]
}
const with_curr_data = (func) => {
    let data = curr_win_data();
    if (data) func(data);
}

app.on('open-file', (event, path) => {
    event.preventDefault();
    open_image(path);
})

app.on('menu:file/open', (event) => {
    open_image_dialog();
})

app.on('menu:file/save', async (event) => {
    with_curr_data(async ({window, image}) => {
        await pdpfs.save(image.id, image.path);
        window.setDocumentEdited(await pdpfs.image_is_dirty(image.id));
    })
})

app.on('menu:file/delete', async (event) => {
    with_curr_data(async ({window, image, selected, send}) => {
        for (let file of selected)
            await pdpfs.rm(image.id, file);
        send('pdpfs:refresh-directory-entries', { entries: pdpfs.get_directory_entries(image.id) });
        window.setDocumentEdited(await pdpfs.image_is_dirty(image.id));
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


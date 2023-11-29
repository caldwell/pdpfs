// Copyright © 2023 David Caldwell <david@porkrind.org>

const { app, BrowserWindow, ipcMain, dialog } = require('electron');
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

    data.window = win;
    data.win_id = win.id;

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

app.on('open-file', (event, path) => {
    event.preventDefault();
    open_image(path);
})

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


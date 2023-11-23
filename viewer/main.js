// Copyright © 2023 David Caldwell <david@porkrind.org>

const { app, BrowserWindow, ipcMain, dialog } = require('electron');
const path = require('node:path');
const fs = require('node:fs');
const pdpfs = require(__dirname);

const temp_path = {};

const create_fs_window = (title, id) => {
    const win = new BrowserWindow({
        width: 800,
        height: 600,
        webPreferences: {
            preload: path.join(__dirname, 'preload.js')
        },
        title: title,
    })

    win.loadFile('web/index.html', { query: { id: id } })
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

    // Sadly because of the way Electron drag and drop works, we _have_ to have the file ready to go
    temp_path[id] = fs.mkdtempSync(path.join(app.getPath("temp"), "image-XXXXXXXX"));
    pdpfs.extract_to_path(id, temp_path[id]);

    create_fs_window(`RT-11: ${image_path}`, id);
}

ipcMain.handle('pdpfs:get_directory_entries', async (event, id) => pdpfs.get_directory_entries(id));

ipcMain.handle('pdpfs:cp_into_image', async (event, id, path) => pdpfs.cp_into_image(id, path));

ipcMain.on('ondragstart', (event, image_id, filenames) => {
    console.log(`dragging [${image_id}] ${temp_path[image_id]}/{${filenames.join(',')}}...`);
    event.sender.startDrag({
        files: filenames.map(f => path.join(temp_path[image_id], f)),
        icon: path.join(__dirname, filenames.length == 1 ? 'web/stack-96.png' : 'web/stack-96.png'),
    })
})

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


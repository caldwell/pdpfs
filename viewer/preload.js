// Copyright © 2023 David Caldwell <david@porkrind.org>

const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('pdpfs', {
    get_directory_entries: (id) => ipcRenderer.invoke('pdpfs:get_directory_entries', id),
    cp_into_image: (id, path) => ipcRenderer.invoke('pdpfs:cp_into_image', id, path),
    open_file: () => ipcRenderer.invoke('dialog:openFile'),
    start_drag: (id, file_names) => ipcRenderer.send('ondragstart', id, file_names),
})
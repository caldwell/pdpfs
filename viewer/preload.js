// Copyright © 2023 David Caldwell <david@porkrind.org>

const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('pdpfs', {
    get_directory_entries: ()           => ipcRenderer.invoke('pdpfs:get_directory_entries'),
    cp_into_image:         (path)       => ipcRenderer.invoke('pdpfs:cp_into_image', path),
    open_file:             ()           => ipcRenderer.invoke('dialog:openFile'),
    start_drag:            (file_names) => ipcRenderer.send('ondragstart', file_names),
    image_is_dirty:        ()           => ipcRenderer.invoke('pdpfs:image_is_dirty'),
    mv:                    (src, dest)  => ipcRenderer.invoke('pdpfs:mv', src, dest),
    rm:                    (filename)   => ipcRenderer.invoke('pdpfs:rm', filename),
    save:                  ()           => ipcRenderer.invoke('pdpfs:save'),
})

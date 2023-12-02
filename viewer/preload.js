// Copyright © 2023 David Caldwell <david@porkrind.org>

const { contextBridge, ipcRenderer } = require('electron')

const get_hacky_args = () => {
    try { return JSON.parse(process.argv.pop()) }
    catch(e) { return  {} }
};

contextBridge.exposeInMainWorld('pdpfs', {
    ...get_hacky_args(),

    get_directory_entries: ()           => ipcRenderer.invoke('pdpfs:get_directory_entries'),
    cp_into_image:         (path)       => ipcRenderer.invoke('pdpfs:cp_into_image', path),
    open_file:             ()           => ipcRenderer.invoke('dialog:openFile'),
    start_drag:            (file_names) => ipcRenderer.send('ondragstart', file_names),
    image_is_dirty:        ()           => ipcRenderer.invoke('pdpfs:image_is_dirty'),
    mv:                    (src, dest)  => ipcRenderer.invoke('pdpfs:mv', src, dest),
    rm:                    (...files)   => ipcRenderer.invoke('pdpfs:rm', ...files),
    save:                  ()           => ipcRenderer.invoke('pdpfs:save'),
    set_selected:          (selected)   => ipcRenderer.send('app:set_selected', selected),
    // New Image Dialog
    cancel:                ()           => ipcRenderer.send('new:cancel'),
    create:                (image_type, device_type, image_size, filesystem) =>
                                           ipcRenderer.send('new:create', image_type, device_type, image_size, filesystem),
})

ipcRenderer.on('pdpfs', (e, type, detail) => window.dispatchEvent(new CustomEvent(type, { detail:detail })))

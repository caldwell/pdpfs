// Copyright © 2023 David Caldwell <david@porkrind.org>

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { jsr } from '@caldwell/jsml/jsml-react.mjs'
import { DndProvider, useDrop } from 'react-dnd'
import { HTML5Backend, NativeTypes } from 'react-dnd-html5-backend'

import './pdpfs.css'

const location=new URL(window.location.href);

async function main() {
    init_tauri();
    let [app_el] = ["pdpfs"].map(id => document.getElementById(id));
    let app_root = createRoot(app_el);
    app_root.render(jsr([DndProvider, {backend:HTML5Backend}, [app]]));

}

window.addEventListener("DOMContentLoaded", main);

let emit, listen;
async function init_tauri() {
    if (!window.__TAURI__) return; // Not running under tauri!
    const { invoke } = window.__TAURI__.primitives;
    const appWindow = window.__TAURI__.window.getCurrent();
    try {
        ({ emit, listen } = await import('@tauri-apps/api/event'));
    } catch(e) {
        console.log("Shouldn't happen", e);
    }

    window.pdpfs = {
        get_directory_entries: async function(image_id) {
            return await invoke("get_directory_entries", { id: image_id });
        },
        cp_into_image: async function (image_id, path) {
            await invoke("cp_into_image", { id: image_id, path: path });
        }
    };
}

function prevent_default(f) {
    return (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (f) return f(e);
    }
}

// Number(null) => 0. Sigh.
const to_num = (s) => s == undefined ? undefined : Number(s);

function app() {
    const image_id = to_num(location.searchParams.get("id"));
    const new_image = location.searchParams.get('kind')=='new';

    return jsr(new_image ? [NewImageSetup] : [DiskImageView, { image_id: image_id }]);
}

function DiskImageView({image_id}) {
    const [entries, set_entries] = React.useState([]);
    const [error, set_error] = React.useState(null);

    let { selected_values, make_selectable, clear_selection } = useSelection(React.useCallback(new_selection => pdpfs.set_selected(new_selection),[]));

    React.useEffect(() => {
        let cancelled;
        pdpfs.get_directory_entries()
            .then((entries) => { if (!cancelled) set_entries(entries) } )
            .catch((err) => set_error(err));
        let handler = (event) => {
            set_entries(event.detail.entries);
            clear_selection(); // A bit broad, but safe. Otherwise we need to correlate by name which we don't do.
        };
        window.addEventListener("pdpfs:refresh-directory-entries", handler);
        return () => { cancelled=true; window.removeEventListener("pdpfs:refresh-directory-entries", handler) };
    }, [image_id, clear_selection]);

    // This drop stuff halfway works on tauri: it lets us do the hovering stuff, but the drop part doesn't
    // work. On electron, the drop _does_ work.
    const [{hovering}, drop] = useDrop(() => ({
        accept: NativeTypes.FILE,
        drop: (drop_obj, _monitor) => {
            for (let file of drop_obj.files)
                try {
                    pdpfs.cp_into_image(file.path)
                } catch(e) {
                    set_error(e);
                    break;
                }
            (async () => { set_entries(await pdpfs.get_directory_entries()) })();
            return { yo:"yo" }
        },
        collect: (monitor) => ({ hovering: monitor.isOver() }),
    }), [])

    const hover_ref = React.useRef(false);
    hover_ref.current = hovering;

    if (listen) { // Tauri has a separate app level listen event for system drag and drops
        React.useEffect(() => { // eslint-disable-line react-hooks/rules-of-hooks
            let canceled = false;
            listen('tauri://file-drop', async event => {
                if (canceled) return;
                if (!hover_ref.current) return;
                for (let path of event.payload.paths)
                    try {
                        pdpfs.cp_into_image(path)
                    } catch(e) {
                        set_error(e);
                        break;
                    }
                set_entries(await pdpfs.get_directory_entries());
            })
            return () => canceled = true;
        }, [image_id, set_entries]);
    }

    let sorted = [...entries].sort((a,b) => a.name > b.name ? 1 : a.name == b.name ? 0 : -1);

    const [editing, set_editing] = React.useState(null);
    const rename = async (src, dest) => {
        try {
            if (src != dest)
                await pdpfs.mv(src, dest);
            set_editing(null);
        } catch(e) {
            // Maybe a tooltip??
            console.log(e);
        }
    };
    React.useEffect(() => {
        let handler = (_event) => { let f = selected_values()[0]; if (f) set_editing(f) };
        window.addEventListener("menu:file/rename", handler);
        return () => { window.removeEventListener("menu:file/rename", handler) };
    }, [set_editing, selected_values]);

    return jsr(['div', { className: `directory-list ${hovering ? "hover" : ""}`, ref: drop },
                ['div', { className: 'header' },
                 ['div', { className: 'filename' }, "Filename"],
                 ['div', { className: 'blocks' }, "Block Count"],
                 ['div', { className: 'size' }, "File Size"],
                 ['div', { className: 'date' }, "Creation Date"]],
                ['div', { className: 'body' },
                 make_selectable(sorted.map((e) => ({ value: e.name,
                                                      el: ['div', { draggable: true, className: `direntry` },
                                                           { onDragStart: prevent_default((_event) => {
                                                               return pdpfs.start_drag()
                                                           }),
                                                             onContextMenu: prevent_default((event) => pdpfs.context_menu(selected_values())),
                                                           },
                                                           ['div', { className: 'icon' }, [svg, { icon: "file" }]],
                                                           ['div', { className: 'filename' },
                                                            editing != e.name ? ['span', e.name,
                                                                                 { onClick: () => { set_editing(e.name) } }]
                                                                              : ['input', { type: "text", size: 12, defaultValue: e.name, autoFocus: true },
                                                                                 { onFocus: (event) => event.target.select(),
                                                                                   onBlur: (event) => { rename(e.name, event.target.value) },
                                                                                   onKeyDown: (event) => { if (event.key == "Escape") {
                                                                                                               set_editing(null);
                                                                                                               return false;
                                                                                                           }
                                                                                                           if (event.key == "Return" || event.key == "Enter")
                                                                                                               rename(e.name, event.target.value);
                                                                                                         },
                                                                                 }]],
                                                           ['div', { className: 'blocks' }, e.length],
                                                           ['div', { className: 'size' }, e.length*512],
                                                           ['div', { className: 'date' }, e.creation_date]]
                                                    })))]]);
}

function useSelection(on_change) {
    let values = React.useRef([]);

    const [selection, _set_selection] = React.useState([]);

    const find_span = React.useCallback((selection, i) => {
        let sel = selection.findIndex(span => span.start <= i && i <= span.end);
        return sel == -1 ? undefined : sel;
    }, []);
    const is_selected = React.useCallback(i => find_span(selection, i) != undefined,
                                          [find_span, selection]);
    const set_selection = React.useCallback(f => {
        return _set_selection(current => {
            let new_selection = f(current);
            on_change(values.current.filter((_v,i) => find_span(new_selection, i) != undefined));
            return new_selection;
        })
    }, [on_change, values, _set_selection, find_span]);

    const coalesce_selection_spans = React.useCallback(sel => {
        sel = sel.map((span,i) => ({ span, i })).sort((a,b) => a.span.start - b.span.start);
        for (let i = 0; i < sel.length-1; i++)
            if (sel[i].span.end+1  >= sel[i+1].span.start) {
                sel.splice(i,2,{ i: Math.max(sel[i].i, sel[i+1].i),
                                 span: {
                                     start: sel[i].span.start,
                                     end: Math.max(sel[i].span.end, sel[i+1].span.end),
                                     anchor: sel[i].i > sel[i+1].i ? sel[i].span.anchor : sel[i+1].span.anchor,
                                 }});
                i--; // We deleted an entry so set ourselves back to compensate
            }
        return sel.sort((a,b) => a.i - b.i).map(({span}) => span)
    }, []);

    const set_selected = React.useCallback(i => {
        set_selection(current => {
            return coalesce_selection_spans([...current, { start: i, end: i, anchor: i }]);
        });
    }, [set_selection, coalesce_selection_spans]);

    const toggle_selected = React.useCallback(i => {
        set_selection(current => {
            let span_i = find_span(current, i);
            if (span_i == undefined)
                return coalesce_selection_spans([...current, { start: i, end: i, anchor: i }]);
            let newsel = [...current];
            let {start, end, anchor} = newsel[span_i];
            newsel.splice(span_i,1);
            if (start <= i - 1)
                newsel.push({ start: start, end: i-1, anchor: anchor == end ? i-1 : anchor });
            if (i+1 <= end)
                newsel.push({ start: i+1,   end: end, anchor: anchor == start ? i+1 : anchor });
            return coalesce_selection_spans(newsel);
        })
    }, [set_selection, find_span, coalesce_selection_spans]);

    const set_last_selection_end = React.useCallback(i => {
        set_selection(current => {
            let newsel = [...current];
            if (newsel.length == 0)
                return [{ start: 0, end: i, anchor: 0 }];
            let {anchor} = newsel.pop();
            newsel.push({ start: Math.min(i,anchor), end: Math.max(i,anchor), anchor: anchor });
            return coalesce_selection_spans(newsel);
        })
    }, [set_selection, coalesce_selection_spans]);

    const mouse_state = React.useRef(false);

    return {
        selected_values: React.useCallback(() => values.current.filter((_v, i) => is_selected(i)), [values, is_selected]),
        clear_selection: React.useCallback(() => set_selection(_current => []), [set_selection]),
        make_selectable: (items) => {
            values.current = items.map((item) => item.value);
            let els = items.map((item) => item.el);
            return jsr([React.Fragment,
                        ...els.map((el,i) => {
                            let cno = el.find(v => typeof(v) == 'object' && v.className);
                            if (cno && is_selected(i)) cno.className += " selected";
                            let odso = el.find(v => typeof(v) == 'object' && v.onDragStart);
                            if (odso && is_selected(i)) {
                                let old_on_drag_start = odso.onDragStart;
                                odso.onDragStart = (event) => { if (mouse_state.current != "selecting") old_on_drag_start(event) }
                            }
                            let ocmo = el.find(v => typeof(v) == 'object' && v.onContextMenu);
                            if (ocmo) {
                                let old_on_context_menu = ocmo.onContextMenu;
                                // Context menu does an onMouseDown but never onMouseUps so we need to manually clear our state
                                ocmo.onContextMenu = (event) => { mouse_state.current = undefined; old_on_context_menu(event) }
                            }
                            return ([ ...el, {
                                            onMouseDown: (event) => {
                                                if (event.altKey || event.metaKey) {
                                                    toggle_selected(i);
                                                    mouse_state.current = "selecting-discontiguous";
                                                } else if (event.shiftKey) {
                                                    set_last_selection_end(i);
                                                    mouse_state.current = "selecting";
                                                } else {
                                                    if (is_selected(i))
                                                        mouse_state.current = "clicked-on-selection";
                                                    else {
                                                        set_selection(_current => [{ start: i, end: i, anchor: i }]);
                                                        mouse_state.current = "selecting"
                                                    }
                                                }
                                                if (mouse_state.current.startsWith("selecting"))
                                                    prevent_default()(event);
                                            },
                                            onMouseMove: (_event) => {
                                                if (mouse_state.current == "selecting")
                                                    set_last_selection_end(i);
                                                if (mouse_state.current == "selecting-discontiguous")
                                                    set_selected(i);
                                            },
                                            onMouseUp: (_event) => {
                                                if (mouse_state.current == "clicked-on-selection") {
                                                    set_selection(_current => [{ start: i, end: i, anchor: i }]);
                                                }
                                                mouse_state.current = undefined
                                            },
                            }])
                        })])
        }
    }
}

function from_human(s) {
    let m;
    if ((m = s.match(/^\s*([\d.]+)\s*([kmgtey])?b?\s*$/i))) {
        let unit = m[2] == undefined ? 0 : "bkmgtey".search(m[2].toLowerCase());
        if (unit == -1) return undefined;
        return Number(m[1]) * 1024 ** unit;
    }
    return undefined
}
function human(v) {
    let e = Math.floor(Math.log(v)/Math.log(1024));
    return `${(v/(1024 ** e)).toFixed(v < 1024 ? 0 : 2)} ${"BKMGTEY".charAt(e)}${v < 1024 ? '' : 'B'}`;
}

const device_type_size = Object.fromEntries(pdpfs.device_types.map(dtype => [dtype.name, dtype.bytes]));

function NewImageSetup() {
    const [image_type,  set_image_type]  = React.useState("img");
    const [device_type, set_device_type] = React.useState("rx01");
    const [image_size,  set_image_size]  = React.useState("1MB");
    const [filesystem,  set_filesystem]  = React.useState("rt11");

    const bytes = from_human(image_size);

    return jsr(['div', { className: "new-image" },
                ['div', { className: "settings" },
                 ['label', "Disk Image Type"],
                 ['select', { defaultValue: "img", onChange: (e) => set_image_type(e.target.value) },
                  pdpfs.image_types.map(imtype => ['option', { value: imtype }, imtype.toUpperCase()])],
                 ['label', "Device Type"],
                  ['select', { defaultValue: "rx01", onChange: (e) => set_device_type(e.target.value) },
                   pdpfs.device_types.map(dtype => ['option', { value: dtype.name }, dtype.name == 'flat' ? 'Custom Sized Image' : `${dtype.name.toUpperCase()} (${human(dtype.bytes)})`])],
                 ['label', "Image Size", device_type != "flat" && { className: "disabled" }],
                 device_type != "flat" ? ['div', { className: "size", key: "read" }, human(device_type_size[device_type])]
                                       : ['div', { className: "size" },
                                          ['input', { type: "text", size: 14, key: "edit", defaultValue: "1 MB",
                                                      className: `${bytes == undefined ? "error" : ""}`,
                                                      onChange: (e) => set_image_size(e.target.value) }],
                                          ['div', { className: "help" }, "Valid forms: 942, 10 M, 10 MB, 10m, 10mb"]],
                 ['label', "File System Format"],
                 ['select', { defaultValue: "rt11", onChange: (e) => set_filesystem(e.target.value) },
                  pdpfs.filesystems.map(fs_type => ['option', { value: fs_type }, fs_type == 'rt11' ? "RT-11" : fs_type.toUpperCase()])]],
                ['div', { className: "buttons" },
                 ['button', { className: "cancel", type: "button" }, "Cancel",
                  { onClick: () => pdpfs.cancel() }],
                 ['button', { className: "ok",     type: "button" }, "Create",
                  device_type == 'flat' && (bytes == undefined || bytes < 20*512) && { disable: "true" },
                  { onClick: () => pdpfs.create(image_type, device_type == 'flat' ? undefined : device_type,
                                                device_type == 'flat' ? bytes : undefined, filesystem) }]]])
}

function svg({icon}) {
    const icons = { file: ['svg', { xmlns:'http://www.w3.org/2000/svg',width:'16',height:'16',fill:'currentColor',className:'bi bi-file-earmark-text',viewBox:'0 0 16 16' },
                           ['path', { d:'M5.5 7a.5.5 0 0 0 0 1h5a.5.5 0 0 0 0-1zM5 9.5a.5.5 0 0 1 .5-.5h5a.5.5 0 0 1 0 1h-5a.5.5 0 0 1-.5-.5m0 2a.5.5 0 0 1 .5-.5h2a.5.5 0 0 1 0 1h-2a.5.5 0 0 1-.5-.5' }],
                           ['path', { d:'M9.5 0H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V4.5zm0 1v2A1.5 1.5 0 0 0 11 4.5h2V14a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1z' }]]
                  };
    return jsr(icons[icon]);
}

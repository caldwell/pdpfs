// Copyright © 2023 David Caldwell <david@porkrind.org>

import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { jsr } from '@caldwell/jsml/jsml-react.mjs'
import { DndProvider, useDrag, useDrop } from 'react-dnd'
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

    return jsr(image_id == undefined ? ['div', "Loading Disk Image..."] : [diskimageview, { image_id: image_id }]);
}

function diskimageview({image_id}) {
    const [entries, set_entries] = React.useState([]);
    const [error, set_error] = React.useState(null);

    React.useEffect(() => {
        (async function() {
            try {
                const entries = await pdpfs.get_directory_entries();
                debugger;
                console.log("setting entries", entries);
                set_entries(entries);
            } catch(e) {
                set_error(e);
            }
        })()
    }, [image_id, set_entries]);

    // This drop stuff halfway works on tauri: it lets us do the hovering stuff, but the drop part doesn't
    // work. On electron, the drop _does_ work.
    const [{hovering}, drop] = useDrop(() => ({
        accept: NativeTypes.FILE,
        drop: (drop_obj, monitor) => {
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
        React.useEffect(() => {
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

    const [selection, _set_selection] = React.useState([]);
    function set_selection(f_or_new) {
        if (typeof f_or_new == 'function')
            return _set_selection(current => {
                let new_selection = f_or_new(current);
                pdpfs.set_selected(sorted.filter((e,i) => find_span(new_selection, i) != undefined).map((e) => e.name));
                return new_selection;
            })
        else {
            pdpfs.set_selected(sorted.filter((e,i) => find_span(f_or_new, i) != undefined).map((e) => e.name));
            return _set_selection(f_or_new);
        }
    }

    function find_span(selection, i) {
        let sel = selection.findIndex(span => span.start <= i && i <= span.end);
        return sel == -1 ? undefined : sel;
    }
    function is_selected(i) {
        return find_span(selection, i) != undefined;
    }
    function set_selected(i) {
        set_selection(current => {
            return coalesce_selection_spans([...current, { start: i, end: i, anchor: i }]);
        });
    }
    function toggle_selected(i) {
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
    }
    function set_last_selection_end(i) {
        set_selection(current => {
            let newsel = [...current];
            if (newsel.length == 0)
                return [{ start: 0, end: i, anchor: 0 }];
            let {start, end, anchor} = newsel.pop();
            newsel.push({ start: Math.min(i,anchor), end: Math.max(i,anchor), anchor: anchor });
            return coalesce_selection_spans(newsel);
        })
    }
    function coalesce_selection_spans(sel) {
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
        let r = sel.sort((a,b) => a.i - b.i).map(({span,i}) => span)
        return r
    }

    const mouse_state = React.useRef(false);

    return jsr(['div', { className: `directory-list ${hovering ? "hover" : ""}`, ref: drop },
                ['div', { className: 'header' },
                 ['div', { className: 'filename' }, "Filename"],
                 ['div', { className: 'blocks' }, "Block Count"],
                 ['div', { className: 'size' }, "File Size"],
                 ['div', { className: 'date' }, "Creation Date"]],
                ...sorted.map((e,i) => ['div', { draggable: true, className: `direntry ${is_selected(i) ? "selected" : ""}`},
                                        {
                                            onMouseDown: (event) => {
                                                if (event.altKey || event.metaKey) {
                                                    toggle_selected(i);
                                                    mouse_state.current = "selecting-discontiguous";
                                                } else if (event.shiftKey) {
                                                    set_last_selection_end(i);
                                                    mouse_state.current = "selecting";;
                                                } else {
                                                    if (is_selected(i))
                                                        mouse_state.current = "clicked-on-selection";
                                                    else {
                                                        set_selection([{ start: i, end: i, anchor: i }]);
                                                        mouse_state.current = "selecting"
                                                    };
                                                }
                                                if (mouse_state.current.startsWith("selecting"))
                                                    prevent_default()(event);
                                            },
                                            onMouseMove: (event) => {
                                                if (mouse_state.current == "selecting")
                                                    set_last_selection_end(i);
                                                if (mouse_state.current == "selecting-discontiguous")
                                                    set_selected(i);
                                            },
                                            onMouseUp: (event) => {
                                                if (mouse_state.current == "clicked-on-selection") {
                                                    set_selection([{ start: i, end: i, anchor: i }]);
                                                }
                                                mouse_state.current = undefined
                                            },
                                            onDragStart: prevent_default((event) => {
                                                if (mouse_state.current != "selecting")
                                                    return pdpfs.start_drag(sorted.filter((e,i) => is_selected(i)).map(e => e.name))
                                                return false;
                                            })
                                        },
                                        ['div', { className: 'filename' }, e.name],
                                        ['div', { className: 'blocks' }, e.length],
                                        ['div', { className: 'size' }, e.length*512],
                                        ['div', { className: 'date' }, e.creation_date],
                                       ])]);
}

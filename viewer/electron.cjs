// Hackery because the electron main uses commonjs require() to load us.
(async() => { try { return await import("./main.js") } catch(e) { console.log("couldn't import main:", e) } })()

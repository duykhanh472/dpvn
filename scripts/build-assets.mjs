import * as esbuild from "esbuild";
import * as fs from "node:fs";
import * as path from "node:path";
import * as crypto from "node:crypto";

const STATIC_DIR = "static";
const JS_OUT = path.join(STATIC_DIR, "js");
const CSS_OUT = path.join(STATIC_DIR, "css");
const MANIFEST_PATH = path.join(STATIC_DIR, "manifest.json");

// Ensure directories exist
fs.mkdirSync(JS_OUT, { recursive: true });
fs.mkdirSync(CSS_OUT, { recursive: true });

// 1. Build JS assets
const jsResult = await esbuild.build({
    entryPoints: ["assets/app.js"],
    bundle: true,
    minify: true,
    sourcemap: true,
    target: ["es2020"],
    outdir: JS_OUT,
    entryNames: "[name].[hash]",
    metafile: true,
});

// 2. Build CSS assets
const cssResult = await esbuild.build({
    entryPoints: ["assets/app.css"],
    bundle: true,
    minify: true,
    sourcemap: true,
    outdir: CSS_OUT,
    entryNames: "[name].[hash]",
    metafile: true,
});

// 3. Ensure HTMX is available in static/js/htmx.min.js
const htmxPath = path.join(JS_OUT, "htmx.min.js");
if (!fs.existsSync(htmxPath)) {
    // Basic fallback/stub if not fetched yet
    fs.writeFileSync(
        htmxPath,
        "// HTMX library stub (download official htmx.min.js in production)\nconsole.debug('htmx stub loaded');\n"
    );
}

// 4. Generate manifest.json
const manifest = {
    "htmx.min.js": "/static/js/htmx.min.js",
};

for (const [outputPath, meta] of Object.entries(jsResult.metafile.outputs)) {
    if (meta.entryPoint === "assets/app.js" && outputPath.endsWith(".js")) {
        manifest["app.js"] = "/" + outputPath.replace(/\\/g, "/");
    }
}

for (const [outputPath, meta] of Object.entries(cssResult.metafile.outputs)) {
    if (meta.entryPoint === "assets/app.css" && outputPath.endsWith(".css")) {
        manifest["app.css"] = "/" + outputPath.replace(/\\/g, "/");
    }
}

fs.writeFileSync(MANIFEST_PATH, JSON.stringify(manifest, null, 2) + "\n");
console.log("Built assets with manifest:", manifest);

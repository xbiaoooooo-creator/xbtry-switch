import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
const app = path.resolve(__dirname, "..");
export default defineConfig({
 root: path.join(app,"src"),
 plugins: [react(), { name:"preview-ipc", transformIndexHtml(html) {
 const mock="/@fs/"+path.resolve(__dirname,"mock.ts").replace(/\\/g,"/");
 return html.replace(/<meta[^>]+Content-Security-Policy[^>]*>/i,"").replace("<head>",`<head><script type="module" src="${mock}"></script>`);
 }}],
 server:{port:4320,strictPort:true,fs:{allow:[app]}},
 resolve:{alias:{"@":path.join(app,"src")}}
});

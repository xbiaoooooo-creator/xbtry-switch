// Development preview only. This file is never included by the production build.
import { getResponse } from "msw";
import { handlers } from "../tests/msw/handlers";
import { setSettings } from "../tests/msw/state";
setSettings({ language:"zh", firstRunNoticeConfirmed:true, newLayoutNoticeConfirmed:true } as never);
let seq=1;
const w=window as unknown as Record<string,unknown>;
w.__TAURI_INTERNALS__={
 metadata:{currentWindow:{label:"main"}, currentWebview:{windowLabel:"main",label:"main"}},
 transformCallback:(cb:unknown)=>{ const id=seq++; w["_"+id]=cb; return id;},
 unregisterCallback:()=>{}, convertFileSrc:(s:string)=>s,
 invoke:async(cmd:string,args:unknown)=>{
 if(cmd.startsWith("plugin:event"))return seq++;
 if(cmd==="plugin:app|version")return "1.1.2";
 if(cmd==="get_settings"){}
 const response=await getResponse(handlers,new Request(`http://tauri.local/${cmd}`,{method:"POST",body:JSON.stringify(args??{})}));
 if(!response)return null;
 const value=await response.text(); if(!response.ok)throw new Error(value);
 return value?JSON.parse(value):undefined;
 }
};
w.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:()=>{}};

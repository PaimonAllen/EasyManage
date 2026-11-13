import {createContext,useContext,useEffect,useId,useRef,useState,type ReactNode,type SyntheticEvent} from "react";
import {installScanBackHistory} from "../lib/scanBackHistory";
type Step="up"|"root"|"pending";
class BackNavigation{
  targets=new Map<string,{step:()=>Step;priority:number}>();active:string|undefined;pending=0;leaving:(()=>void)|undefined;stopped=false;draining=false;
  register(id:string,step:()=>Step,nested:boolean,fallback:boolean){
    const priority=fallback?-1:nested?1:0;
    this.targets.set(id,{step,priority});if(!this.active||nested||(this.targets.get(this.active)?.priority??-1)<priority)this.active=id;
    this.changed();
    return()=>{this.targets.delete(id);if(this.active===id)this.active=[...this.targets.keys()].at(-1);};
  }
  activate(id:string){if(this.targets.has(id))this.active=id;}
  back(leave:()=>void){this.leaving=leave;this.pending=Math.min(64,this.pending+1);this.changed();}
  changed(){
    if(this.stopped||this.draining)return;
    this.draining=true;
    try{
      while(this.pending){
        const step=(this.active?this.targets.get(this.active)?.step():undefined)??"root";
        if(step==="pending")break;
        this.pending--;
        if(step==="root"){this.pending=0;this.leaving?.();break;}
      }
    }finally{this.draining=false;}
  }
}
const Context=createContext<BackNavigation|null>(null);
export function ScanBackNavigation({children}:{children:ReactNode}){
  const [control]=useState(()=>new BackNavigation());
  useEffect(()=>{
    control.stopped=false;
    const dispose=installScanBackHistory(leave=>control.back(leave));
    return()=>{control.stopped=true;control.pending=0;control.leaving=undefined;dispose();};
  },[control]);
  return <Context.Provider value={control}>{children}</Context.Provider>;
}
export function useScanBackView(step:()=>Step,nested=false,fallback=false){
  const control=useContext(Context);const id=useId();const latest=useRef(step);latest.current=step;
  useEffect(()=>control?.register(id,()=>latest.current(),nested,fallback),[control,id,nested,fallback]);
  useEffect(()=>{control?.changed();},[control,step]);
  const activate=()=>control?.activate(id);
  const event=(event:SyntheticEvent<HTMLElement>)=>{
    const target=event.target as Element;
    if(typeof target.closest==="function"&&target.closest("[data-scan-back-view]")===event.currentTarget)activate();
  };
  return {activate,props:{"data-scan-back-view":id,onPointerDown:event,onClickCapture:event,onFocusCapture:event}};
}

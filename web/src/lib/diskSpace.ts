import type {SpaceEntry,SpacePage} from "../api/diskSpace";
export type SpaceBasis="allocated"|"apparent";
export type SpaceSort="size"|"percent"|"name"|"files";
export const excludedSpaceEntry=(entry:SpaceEntry)=>["mount_boundary","duplicate_mount","separate_filesystem"].includes(entry.issue??"");
export function spaceDirectoryPath(ancestors:SpaceEntry[]):string {
  let path="";
  for(const entry of ancestors)path=entry.scan_path??`${path}/${entry.name}`;
  return path.replace(/\/{2,}/g,"/");
}
export function parentPercent(entry:SpaceEntry,nodes:SpaceEntry[],basis:SpaceBasis):number|null {
  if(entry.parent===null||!nodes[entry.parent]||nodes[entry.parent][basis]<=0)return null;
  return Math.min(100,100*entry[basis]/nodes[entry.parent][basis]);
}
export function entryPath(entry:SpaceEntry,nodes:SpaceEntry[]):string {
  const parts:string[]=[];let current=entry;
  for(let count=0;count<nodes.length;count++){
    parts.unshift(current.scan_path??current.name);
    if(current.scan_path)break;
    if(current.parent===null)break;
    current=nodes[current.parent];if(!current)break;
  }
  return parts.join("/").replace(/\/{2,}/g,"/");
}
export function sortedEntries(entries:SpaceEntry[],nodes:SpaceEntry[],basis:SpaceBasis,sort:SpaceSort,ascending:boolean):SpaceEntry[]{
  return [...entries].sort((a,b)=>{
    const value=sort==="name"?a.name.localeCompare(b.name):sort==="files"?a.files-b.files:sort==="percent"?(parentPercent(a,nodes,basis)??-1)-(parentPercent(b,nodes,basis)??-1):a[basis]-b[basis];
    return (ascending?value:-value)||a.name.localeCompare(b.name)||a.id-b.id;
  });
}
export interface SpaceRect {id:number;name:string;value:number;x:number;y:number;width:number;height:number}
export function directorySummaryItems(page:SpacePage,basis:SpaceBasis):{id:number;name:string;value:number}[]{
  const items=page.top.map(e=>({id:e.id,name:e.name,value:e[basis]}));
  if(page.direct&&page.direct[basis]>0)items.push({id:-2,name:"本层文件及目录自身",value:page.direct[basis]});
  const rest=Math.max(0,page.directory[basis]-items.reduce((sum,e)=>sum+e.value,0));
  if(rest>0)items.push({id:-1,name:page.direct?"其他子目录":"其他条目 / 目录本身",value:rest});
  return items;
}
// Balanced binary treemap: rectangle area is proportional to measured bytes.
export function spaceTreemap(items:{id:number;name:string;value:number}[],width=1000,height=300):SpaceRect[]{
  const result:SpaceRect[]=[];
  function partition(list:typeof items,x:number,y:number,w:number,h:number){
    if(!list.length)return;
    if(list.length===1){result.push({...list[0],x,y,width:w,height:h});return;}
    const total=list.reduce((n,item)=>n+item.value,0);let subtotal=0,index=0;
    while(index<list.length-1&&(index===0||subtotal<total/2)){subtotal+=list[index].value;index++;}
    const ratio=subtotal/total;
    if(w>=h){partition(list.slice(0,index),x,y,w*ratio,h);partition(list.slice(index),x+w*ratio,y,w*(1-ratio),h);}
    else{partition(list.slice(0,index),x,y,w,h*ratio);partition(list.slice(index),x,y+h*ratio,w,h*(1-ratio));}
  }
  partition(items.filter(item=>Number.isFinite(item.value)&&item.value>0).sort((a,b)=>b.value-a.value),0,0,width,height);
  return result;
}

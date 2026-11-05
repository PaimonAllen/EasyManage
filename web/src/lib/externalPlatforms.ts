// Parse locally only: adding a shortcut never fetches or probes its destination.
export function externalPlatformUrl(value:string):string|null {
  const input=value.trim();
  if(input.length>4096||/[\u0000-\u001f\u007f-\u009f]/u.test(input))return null;
  try{
    const url=new URL(input);
    return ["http:","https:"].includes(url.protocol)&&url.hostname&&!url.username&&!url.password&&url.href.length<=4096?url.href:null;
  }catch{return null;}
}

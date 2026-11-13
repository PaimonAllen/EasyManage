// Mirrors the conservative server policy for the UI; the server enforces it.
export function sensitiveScanPath(path:string,scope:string,windows=false):boolean {
  if(scope==="all_local")return true;
  const parts=(windows?path.replaceAll("\\","/").slice(3):path).split("/").filter(Boolean);
  if(parts.length<=1)return true;
  return windows?["windows","program files","program files (x86)","programdata","system volume information","recovery","$recycle.bin"].includes(parts[0].toLowerCase()):["root","etc","boot","usr","var","bin","sbin","lib","lib32","lib64","libx32","proc","sys","dev","run","opt","srv"].includes(parts[0]);
}

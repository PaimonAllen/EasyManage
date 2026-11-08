import type {ProxyProfile} from "../api/proxyLab";

export function sameNameProfiles(name:string,profiles:ProxyProfile[]){
  const incoming=name.trim();
  return incoming?profiles.filter(profile=>profile.name===incoming):[];
}

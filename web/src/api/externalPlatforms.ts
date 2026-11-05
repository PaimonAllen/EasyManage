export interface ExternalPlatform {
  id:string; name:string; url:string; note:string; revision:number;
  created_at:string; updated_at:string; deleted_at:string|null;
}
export interface ExternalPlatformInput {name:string;url:string;note:string}
const ENDPOINT="/api/v1/external-platforms";
async function response<T>(reply:Response):Promise<T>{
  if(!reply.ok){
    let message=`外部平台接口异常（HTTP ${reply.status}）`;
    try{const error=await reply.json() as {message?:string};message=error.message??message;}catch{/* Keep status for non-JSON replies. */}
    throw new Error(message);
  }
  return reply.json() as Promise<T>;
}
export async function fetchExternalPlatforms(signal:AbortSignal):Promise<ExternalPlatform[]>{
  return (await response<{platforms:ExternalPlatform[]}>(await fetch(ENDPOINT,{cache:"no-store",signal}))).platforms;
}
export async function saveExternalPlatform(input:ExternalPlatformInput,current?:ExternalPlatform):Promise<ExternalPlatform>{
  return response(await fetch(current?`${ENDPOINT}/${encodeURIComponent(current.id)}`:ENDPOINT,{
    method:current?"PUT":"POST",headers:{"Content-Type":"application/json"},cache:"no-store",signal:AbortSignal.timeout(10_000),
    body:JSON.stringify({...input,...(current?{expected_revision:current.revision}:{})}),
  }));
}
export async function changeExternalPlatform(current:ExternalPlatform,action:"delete"|"restore"):Promise<ExternalPlatform>{
  return response(await fetch(`${ENDPOINT}/${encodeURIComponent(current.id)}/manage`,{
    method:"POST",headers:{"Content-Type":"application/json"},cache:"no-store",signal:AbortSignal.timeout(10_000),
    body:JSON.stringify({action,confirmation:current.id,expected_revision:current.revision}),
  }));
}

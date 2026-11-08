export function localNetworkState(state:string|undefined){
  return state==="up"?"网络可达":state==="slow"?"网络可达 · 缓慢":state==="down"?"HTTPS 未连通":state==="unknown"?"检测未确认":"等待检测";
}
export function localHttpStatus(status:number|null|undefined){
  if(status==null)return "尚未收到 HTTP 响应";
  if(status===404)return "HTTP 404 · 网站路径未找到";
  if(status===405)return "HTTP 405 · 请求方法不被支持";
  if(status===429)return "HTTP 429 · 网站限流";
  if(status===501)return "HTTP 501 · 网站未实现该请求方法";
  if([401,403,412,418].includes(status))return `HTTP ${status} · 网站访问受限`;
  if(status>=500)return `HTTP ${status} · 网站服务异常`;
  if(status>=400)return `HTTP ${status} · 网站返回错误`;
  if(status>=300)return `HTTP ${status} · 网站重定向`;
  return `HTTP ${status}`;
}
export function localHttpFallback(value:{head_status:number;get_status:number|null}|null|undefined){
  if(!value)return null;
  return value.get_status==null?`HEAD ${value.head_status} · GET 兼容复核未取得响应`:`兼容检测：HEAD ${value.head_status} → GET ${value.get_status} · 仅读取响应头`;
}

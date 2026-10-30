import {useCallback,useEffect,useId,useMemo,useRef,useState} from "react";
import {cancelSpaceScan,fetchSpaceScan,fetchSpaceStatus,startSpaceScan,type SpaceEntry,type SpaceResult,type SpaceScan,type SpaceStatus} from "../api/diskSpace";
import {entryPath,parentPercent,sortedEntries,spaceTreemap,type SpaceBasis,type SpaceSort} from "../lib/diskSpace";
import {formatBytes} from "../lib/format";
import {capacityKnown,isBootMount,scanDiskTargets} from "../lib/storageMounts";
import type {NodeSummary} from "../types/monitoring";
import {AdminCredentialFields,useAdminCredentials} from "./AdminCredentials";
import {PagedSpaceVisualization} from "./PagedSpaceVisualization";
import "./DiskSpaceExplorer.css";

const statusText:Record<string,string>={running:"扫描中",complete:"扫描完成",partial:"不完整快照",cancelled:"已取消（部分结果）",failed:"扫描失败",interrupted:"平台重启，扫描中断"};
const issueText:Record<string,string>={permission_denied:"无权读取",changed_or_denied:"条目变化或无法读取",unrepresentable_name:"文件名编码不支持",path_limit:"路径长度达到限制",depth_limit:"达到层级限制",hardlink:"硬链接，已在别处计数",symlink:"符号链接，不跟随",mount_boundary:"其他挂载点，未扫描",special:"特殊文件，不扫描内容"};
const reasonText:Record<string,string>={time_limit:"达到所选扫描时限",entry_limit:"达到所选条目上限",cancelled:"手动取消",unreadable_entries:"部分目录无权读取或层级受限",platform_storage_low:"平台结果存储空间不足，已停止扫描以保护数据库",platform_storage_unknown:"无法确认平台存储可用空间，已安全停止扫描",memory_limit:"扫描内存达到保护上限"};

export function DiskSpaceExplorer({nodes,showBoot=true}:{nodes:NodeSummary[];showBoot?:boolean}){
  const [selected,setSelected]=useState("");
  const node=nodes.find(n=>n.id===selected)??nodes[0];
  return <article className="data-panel disk-explorer">
    <header className="panel-header"><div><h2>磁盘空间分析</h2><p>按目录层级定位空间占用 · 手动扫描 · 只读</p></div>
      <label className="disk-explorer__machine">选择机器<select value={node?.id??""} onChange={e=>setSelected(e.target.value)} aria-label="磁盘空间分析机器">
        {nodes.map(n=><option key={n.id} value={n.id}>{n.hostname}{n.online?"":" · 离线"}</option>)}
      </select></label></header>
    {node?<MachineSpace key={node.id} node={node} showBoot={showBoot}/>:<p className="table-empty">暂无机器。</p>}
  </article>;
}

function RootAuthorization({node,onClose,onReady}:{node:NodeSummary;onClose:()=>void;onReady:()=>void}){
  const auth=useAdminCredentials([node.id]);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  const dialog=useRef<HTMLDialogElement>(null);
  useEffect(()=>{dialog.current?.showModal();return()=>dialog.current?.close();},[]);
  return <dialog ref={dialog} className="credentials-dialog disk-explorer__auth" aria-label="root 磁盘扫描授权" onCancel={e=>{if(busy)e.preventDefault();else onClose();}}><form onSubmit={e=>{
    e.preventDefault();setBusy(true);setError("");void auth.prepare().then(onReady).catch(e=>setError(e.message)).finally(()=>setBusy(false));
  }}><div className="dialog-title"><h2>root 磁盘扫描 · {node.hostname}</h2><button type="button" className="icon-button" disabled={busy} onClick={onClose} aria-label="关闭">×</button></div>
    <p className="hint">使用现有管理员授权读取目录占用，root 扫描快照也仅对已授权浏览器显示。</p>
    <AdminCredentialFields auth={auth} disabled={busy}/>{error&&<p className="form-error" role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" className="button button--secondary" disabled={busy} onClick={onClose}>取消</button><button className="button" disabled={busy||!auth.ready}>{busy?"验证中…":"验证并保存授权"}</button></div>
  </form></dialog>;
}

function MachineSpace({node,showBoot}:{node:NodeSummary;showBoot:boolean}){
  const [status,setStatus]=useState<SpaceStatus|null>(null);const [scan,setScan]=useState<SpaceScan|null>(null);
  const [selected,setSelected]=useState("");const [path,setPath]=useState("");const [mode,setMode]=useState<"basic"|"full">("basic");
  const [scope,setScope]=useState<"directory"|"disk"|"all_local">("directory");
  const [diskKey,setDiskKey]=useState("");
  const diskTargets=useMemo(()=>scanDiskTargets(node.metrics?.disks??[],node.metrics?.block_devices??[],showBoot),[node.metrics?.disks,node.metrics?.block_devices,showBoot]);
  const selectedDisk=diskTargets.find(target=>target.key===diskKey);
  const scanPath=scope==="disk"?selectedDisk?.disk.mount_point??"":scope==="all_local"?"/":path;
  const choosePath=(value:string)=>{setPath(value);setScope("directory");};
  const [seconds,setSeconds]=useState(0);const [maxEntries,setMaxEntries]=useState(0);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  const [authorize,setAuthorize]=useState(false);const [refresh,setRefresh]=useState(0);
  const loaded=useRef("");
  const full=node.metrics?.privilege?.mode==="full"&&node.metrics.privilege.uid===0;
  useEffect(()=>{
    const controller=new AbortController();let timer:ReturnType<typeof setTimeout>;
    async function load(){
      try{
        const next=await fetchSpaceStatus(node.id,controller.signal);if(controller.signal.aborted)return;
        setStatus(next);setPath(current=>current||next.default_path||"/");
        const id=next.scans.some(s=>s.id===selected)?selected:next.scans[0]?.id;
        const summary=next.scans.find(s=>s.id===id);
        if(summary?.status==="running"){setScan(summary);loaded.current="";}
        else if(summary){
          const key=`${summary.id}:${summary.status}:${summary.finished_at}`;
          if(loaded.current!==key){const detail=await fetchSpaceScan(node.id,summary.id,controller.signal);if(!controller.signal.aborted){setScan(detail);loaded.current=key;}}
        }else{setScan(null);loaded.current="";}
        if(!controller.signal.aborted)timer=setTimeout(()=>{if(document.hidden){timer=setTimeout(load,15000);}else void load();},next.scans.some(s=>s.status==="running")?3000:15000);
      }catch(e){if(!controller.signal.aborted){setError(e instanceof Error?e.message:"读取扫描记录失败");setScan(null);loaded.current="";timer=setTimeout(load,15000);}}
    }
    void load();return()=>{controller.abort();clearTimeout(timer);};
  },[node.id,selected,refresh]);
  const running=status?.scans.some(s=>s.status==="running");
  async function start(){
    if(scope==="disk"&&!selectedDisk){setError("请选择当前机器的已挂载本地磁盘");return;}
    setBusy(true);setError("");
    try{
      // A disk choice is a scan rooted at its mounted filesystem; the server
      // revalidates the path and mount boundaries using the existing scanner.
      const result=await startSpaceScan(node.id,scanPath,mode,seconds,maxEntries,scope==="all_local"?"all_local":"directory");
      setSelected(result.id);setRefresh(v=>v+1);
    }catch(e){setError(e instanceof Error?e.message:"扫描启动失败");}finally{setBusy(false);}
  }
  async function cancel(){if(!scan)return;setBusy(true);try{await cancelSpaceScan(node.id,scan.id);setRefresh(v=>v+1);}catch(e){setError(e instanceof Error?e.message:"取消失败");}finally{setBusy(false);}}
  return <div className="disk-explorer__body">
    <div className="disk-explorer__controls">
      <label>扫描范围<select value={scope} onChange={e=>setScope(e.target.value as typeof scope)} disabled={busy}>
        <option value="directory">指定目录</option><option value="disk">指定磁盘</option><option value="all_local">全部本地磁盘</option>
      </select></label>
      <label>扫描级别<select value={mode} onChange={e=>setMode(e.target.value as typeof mode)} disabled={busy}>
        <option value="basic">普通用户 · {status?.username??"原 SSH 账户"}</option><option value="full" disabled={!full}>root · Full Agent{full?"":"（需先提权）"}</option>
      </select></label>
      {scope==="disk"?<label className="disk-explorer__path">选择磁盘 / 分区<select value={selectedDisk?.key??""} onChange={e=>setDiskKey(e.target.value)} disabled={busy||!diskTargets.length} aria-label="选择要扫描的本地磁盘">
        <option value="">{diskTargets.length?"请选择磁盘 / 分区":"暂无可扫描的已挂载本地磁盘"}</option>
        {diskTargets.map(target=><option key={target.key} value={target.key}>{target.disk.name} · {target.disk.mount_point} · {capacityKnown(target.disk)?formatBytes(target.disk.total_bytes):"容量未知"}{target.subtree?" · 子目录挂载":""}</option>)}
      </select></label>:<label className="disk-explorer__path">目录路径<input value={scope==="all_local"?"自动发现已挂载的本地文件系统":path} disabled={scope==="all_local"} onChange={e=>setPath(e.target.value)} placeholder="/home/username" maxLength={2048} spellCheck={false} aria-label="扫描目录绝对路径"/></label>}
      <label>扫描时限<select value={seconds} onChange={e=>setSeconds(Number(e.target.value))}><option value={0}>无限制</option><option value={30}>30 秒</option><option value={60}>60 秒</option><option value={120}>120 秒</option></select></label>
      <label>条目数量<select value={maxEntries} onChange={e=>setMaxEntries(Number(e.target.value))}><option value={0}>无限制</option><option value={20000}>20,000 项</option><option value={100000}>100,000 项</option></select></label>
      {mode==="full"&&!status?.root_authorized?<button className="button" disabled={!full||busy} onClick={()=>setAuthorize(true)}>授权 root 扫描</button>:
        <button className="button" disabled={!node.online||!status||busy||running||!scanPath.startsWith("/")} onClick={()=>void start()}>{busy?"处理中…":"开始扫描"}</button>}
    </div>
    {scope==="disk"&&selectedDisk&&<p className="hint disk-explorer__disk-selection">扫描挂载点：<strong>{selectedDisk.disk.mount_point}</strong> · {selectedDisk.disk.file_system}{selectedDisk.aliases>0&&<> · 已合并 {selectedDisk.aliases} 个重复共享入口</>}{selectedDisk.subtree&&" · 当前仅挂载了文件系统的子目录，扫描只覆盖这个子目录"}</p>}
    {scope==="disk"&&diskKey&&!selectedDisk&&<p className="disk-explorer__notice">原选磁盘已不在当前可选清单中，请重新选择。</p>}
    <div className="disk-explorer__shortcuts"><span>快捷目录</span>
      {[...new Set([status?.default_path,...(node.metrics?.disks??[]).map(d=>d.mount_point)])].filter((p):p is string=>!!p&&(showBoot||!isBootMount(p))).slice(0,16).map(p=><button type="button" key={p} className="button button--secondary button--small" onClick={()=>choosePath(p)}>{p}</button>)}
    </div>
    <p className="hint">时间与条目数量默认无限制，直到扫描完成或手动取消；文件元数据在内存中汇总，仅保存目录大小、数量和层级，不保存文件名及逐文件明细。保持低优先级、只读，每台机器 1 个任务、平台同时 2 个；每个文件系统最大 64 层，不跟随符号链接。关闭页面后任务继续。</p>
    <p className="hint">平台结果存储分区可用空间低于 2 GiB 时，会停止扫描并保存不完整快照；释放空间或迁移数据库后可重新扫描。</p>
    <p className={scope==="all_local"?"disk-explorer__notice":"hint"}>{scope==="all_local"?"一次扫描所有已挂载且支持的本地文件系统，按磁盘分别展示并汇总；绑定挂载去重，网络盘、虚拟文件系统和未挂载分区不扫描。普通用户无权读取的部分会标为不完整。全机扫描可能持续较久，并增加 I/O 和数据库快照占用。":scope==="disk"?"指定磁盘按已挂载的分区或文件系统选择，多分区磁盘分别列出。仅扫描所选挂载点，不跨其他挂载点；网络盘、虚拟文件系统和未挂载分区不列入。启动分区跟随页面显隐设置。":"指定目录仅扫描所在文件系统，跳过其他挂载点；要一次查看各块本地磁盘，请选择「全部本地磁盘」。"}</p>
    {mode==="basic"&&<p className="hint">以 {status?.username??"原 SSH 用户"} 的实际访问权限扫描；受限目录标为未完整读取。普通用户快照遵循平台现有内网监控页面的访问范围。</p>}
    {!node.online&&<p className="disk-explorer__notice">机器离线，仍可查看已保存快照；上线后可重新扫描。</p>}
    {error&&<p className="form-error" role="alert">{error}</p>}
    <div className="disk-explorer__history"><label>扫描记录<select value={scan?.id??""} onChange={e=>setSelected(e.target.value)} disabled={!status?.scans.length}>
      {!status?.scans.length&&<option value="">尚无扫描快照</option>}
      {status?.scans.map(s=><option key={s.id} value={s.id}>{new Date(s.started_at).toLocaleString()} · {s.mode==="full"?"root":"普通用户"} · {s.scope==="all_local"?"全部本地磁盘":s.root_path} · {statusText[s.status]??s.status}</option>)}
    </select></label><span className="hint">每级别保留最近 3 次，最长 7 天</span></div>
    {scan&&<div className="disk-explorer__scan-state" data-status={scan.status} role="status"><strong>{statusText[scan.status]??scan.status}</strong>
      <span>{scan.result?.visited??scan.progress.visited??0} 个条目 · {scan.result?.elapsed_seconds??scan.progress.elapsed_seconds??0} 秒</span>
      {scan.status==="running"&&<><progress aria-label="扫描进行中"/><button className="button button--secondary button--small" disabled={busy} onClick={()=>void cancel()}>取消扫描</button></>}
      {scan.message&&<span className="form-error">{scan.message}</span>}
    </div>}
    {scan?.result?(scan.result.paged?<PagedSpaceVisualization key={scan.id} nodeId={node.id} scanId={scan.id} result={scan.result} onChoosePath={choosePath}/>:<SpaceVisualization key={scan.id} result={scan.result} onChoosePath={choosePath}/>):scan?.status!=="running"&&<p className="table-empty">{scan&&["failed","interrupted"].includes(scan.status)?"本次未生成完整汇总；已写入的条目保留，无法作为完整扫描结果展示。请解决提示的问题后重新扫描。":"选择目录并开始扫描，即可按层级查看空间占用。"}</p>}
    {authorize&&<RootAuthorization node={node} onClose={()=>setAuthorize(false)} onReady={()=>{setAuthorize(false);setRefresh(v=>v+1);}}/>}
  </div>;
}

export function SpaceVisualization({result,onChoosePath}:{result:SpaceResult;onChoosePath:(path:string)=>void}){
  const [directory,setDirectory]=useState(0);const [basis,setBasis]=useState<SpaceBasis>("allocated");
  const [sort,setSort]=useState<SpaceSort>("size");const [ascending,setAscending]=useState(false);
  const [expanded,setExpanded]=useState<Set<number>>(new Set());const [search,setSearch]=useState("");const [limit,setLimit]=useState(250);
  const clipPrefix=useId().replace(/:/g,"");
  const nodes=result.nodes;const current=nodes[directory]??nodes[0];
  const children=useMemo(()=>{const map=new Map<number,SpaceEntry[]>();for(const n of nodes){if(n.parent!==null){const list=map.get(n.parent)??[];list.push(n);map.set(n.parent,list);}}return map;},[nodes]);
  const navigate=useCallback((id:number)=>{if(nodes[id]?.kind!=="directory")return;setDirectory(id);setSearch("");setExpanded(new Set());setLimit(250);},[nodes]);
  const direct=children.get(current.id)??[];
  const sorted=useCallback((entries:SpaceEntry[])=>sortedEntries(entries,nodes,basis,sort,ascending),[nodes,basis,sort,ascending]);
  const rows=useMemo(()=>{const values:{entry:SpaceEntry;depth:number}[]=[];
    function walk(id:number,depth:number){for(const entry of sorted(children.get(id)??[])){
      if(!search||entry.name.toLocaleLowerCase().includes(search.toLocaleLowerCase()))values.push({entry,depth});
      if(expanded.has(entry.id)&&depth<64)walk(entry.id,depth+1);
    }}walk(current.id,0);return values;
  },[children,current.id,expanded,sorted,search]);
  const crumbs:SpaceEntry[]=[];let crumb:SpaceEntry|undefined=current;
  while(crumb){crumbs.unshift(crumb);crumb=crumb.parent===null?undefined:nodes[crumb.parent];}
  const top=[...direct].filter(e=>e[basis]>0).sort((a,b)=>b[basis]-a[basis]);
  const shown=top.slice(0,40).map(e=>({id:e.id,name:e.name,value:e[basis]}));
  const remainder=Math.max(0,current[basis]-shown.reduce((n,e)=>n+e.value,0));
  if(remainder>0)shown.push({id:-1,name:"其他条目 / 目录本身",value:remainder});
  const rectangles=spaceTreemap(shown);
  function sortBy(key:SpaceSort){if(sort===key)setAscending(v=>!v);else{setSort(key);setAscending(key==="name");}setLimit(250);}
  const sortButton=(key:SpaceSort,label:string)=><button className="disk-explorer__sort" onClick={()=>sortBy(key)}>{label} {sort===key?(ascending?"↑":"↓"):"↕"}</button>;
  return <div className="disk-explorer__visual">
    {result.status!=="complete"&&<p className="disk-explorer__notice">{reasonText[result.reason]??"本次扫描未完成"}。大小为已读取部分的下限；百分比仅相对于已扫描数据，不代表完整目录。可选择更小的目录再次扫描。</p>}
    <div className="disk-explorer__summary"><div><span>实际占用</span><strong>{formatBytes(current.allocated)}{current.incomplete?" +":""}</strong></div><div><span>文件大小</span><strong>{formatBytes(current.apparent)}{current.incomplete?" +":""}</strong></div><div><span>文件 / 子目录</span><strong>{current.files.toLocaleString()} / {current.directories.toLocaleString()}</strong></div><div><span>扫描身份</span><strong>{result.username} · UID {result.uid}</strong></div></div>
    <nav className="disk-explorer__breadcrumbs" aria-label="目录层级">{crumbs.map((e,i)=><span key={e.id}>{i>0&&<span aria-hidden="true"> / </span>}<button disabled={e.id===current.id} onClick={()=>navigate(e.id)}>{e.name}</button></span>)}</nav>
    <div className="disk-explorer__toolbar"><label>统计口径<select value={basis} onChange={e=>setBasis(e.target.value as SpaceBasis)}><option value="allocated">实际磁盘占用</option><option value="apparent">文件大小（逻辑大小）</option></select></label>
      <button className="button button--secondary button--small" onClick={()=>{setExpanded(new Set());setLimit(250);}}>折叠全部</button>
      <button className="button button--secondary button--small" onClick={()=>{setExpanded(new Set(direct.filter(e=>e.kind==="directory").map(e=>e.id)));setLimit(250);}}>展开下一层</button>
      <button className="button button--secondary button--small" onClick={()=>onChoosePath(entryPath(current,nodes))}>将此目录设为扫描范围</button>
    </div>
    {rectangles.length>0?<svg className="disk-explorer__treemap" viewBox="0 0 1000 300" aria-label="当前目录空间占用矩形图，面积代表已扫描大小">
      {rectangles.map((rect,i)=>{const entry=nodes[rect.id];const navigable=entry?.kind==="directory"&&entry.issue!=="mount_boundary";const label=`${rect.name} · ${formatBytes(rect.value)} · ${current[basis]>0?(rect.value/current[basis]*100).toFixed(1):0}%`;
        return <g key={rect.id} role={navigable?"button":undefined} tabIndex={navigable?0:undefined} aria-label={label} onClick={()=>navigable&&navigate(rect.id)} onKeyDown={e=>{if(navigable&&(e.key==="Enter"||e.key===" ")){e.preventDefault();navigate(rect.id);}}}>
          <title>{label}</title><defs><clipPath id={`${clipPrefix}-${i}`}><rect x={rect.x+5} y={rect.y+3} width={Math.max(0,rect.width-10)} height={Math.max(0,rect.height-6)}/></clipPath></defs>
          <rect x={rect.x} y={rect.y} width={rect.width} height={rect.height} fill={`var(--space-color-${i%6})`} stroke="var(--surface)" strokeWidth={2}/>
          {rect.width>60&&rect.height>28&&<g clipPath={`url(#${clipPrefix}-${i})`}><text x={rect.x+9} y={rect.y+20}>{rect.name}</text>{rect.height>48&&<text x={rect.x+9} y={rect.y+40} className="disk-explorer__treemap-size">{formatBytes(rect.value)}</text>}</g>}
        </g>;})}
    </svg>:<p className="hint">当前目录没有已读取且大于 0 的占用，或读取范围受限。</p>}
    <div className="disk-explorer__filter"><label>筛选已展开项<input value={search} onChange={e=>{setSearch(e.target.value);setLimit(250);}} placeholder="名称关键字"/></label><span className="hint">点击目录名下钻；箭头展开层级。矩形图展示前 40 项，其余合并。</span></div>
    <div className="table-scroll"><table className="disk-explorer__tree"><thead><tr>
      <th>{sortButton("name","名称 / 层级")}</th><th>{sortButton("size",basis==="allocated"?"实际占用":"文件大小")}</th><th>{sortButton("percent","占上一层")}</th><th>{sortButton("files","文件数")}</th><th>子目录</th><th>状态</th>
    </tr></thead><tbody>{rows.slice(0,limit).map(({entry,depth})=>{const percent=parentPercent(entry,nodes,basis);const hasChildren=children.has(entry.id);const unknown=entry.incomplete&&entry.files===0&&entry.directories===0;return <tr key={entry.id}>
      <td><div className="disk-explorer__entry" style={{paddingLeft:`${Math.min(depth,16)*16}px`}}>
        <button className="disk-explorer__expand" disabled={!hasChildren} aria-label={`${expanded.has(entry.id)?"折叠":"展开"} ${entry.name}`} aria-expanded={hasChildren?expanded.has(entry.id):undefined} onClick={()=>setExpanded(previous=>{const next=new Set(previous);if(next.has(entry.id))next.delete(entry.id);else next.add(entry.id);return next;})}>{hasChildren?(expanded.has(entry.id)?"▾":"▸"):"·"}</button>
        <button className="disk-explorer__name" title={entryPath(entry,nodes)} disabled={entry.kind!=="directory"||entry.issue==="mount_boundary"} onClick={()=>navigate(entry.id)}><span aria-hidden="true">{entry.kind==="directory"?"▰":"▱"} </span>{entry.name}</button>
      </div></td><td className="number">{unknown?"未完整读取":formatBytes(entry[basis])}{entry.incomplete&&!unknown?" +":""}</td>
      <td><div className="disk-explorer__percent"><span>{unknown||percent===null?"—":`${percent.toFixed(1)}%`}</span><div><i style={{width:`${unknown?0:percent??0}%`}}/></div></div></td>
      <td className="number">{entry.files.toLocaleString()}{entry.incomplete?" +":""}</td><td className="number">{entry.directories.toLocaleString()}{entry.incomplete?" +":""}</td>
      <td className={entry.incomplete?"disk-explorer__warning":"hint"}>{entry.issue?(issueText[entry.issue]??entry.issue):entry.incomplete?"下限值":"已读取"}</td>
    </tr>;})}</tbody></table></div>
    {!rows.length&&<p className="table-empty">当前层级没有匹配条目。</p>}{rows.length>limit&&<button className="button button--secondary button--small" onClick={()=>setLimit(n=>n+250)}>继续显示 {Math.min(250,rows.length-limit)} 项（共 {rows.length.toLocaleString()} 项）</button>}
    <p className="hint">{result.filesystem} · {result.errors} 个读取问题 · 跳过 {result.skipped_mounts} 个挂载点 · {result.symlinks} 个符号链接 · {result.hardlinks} 个重复硬链接。硬链接仅首次计入，目录分摊受遍历顺序影响。实际占用来自文件系统分配块；稀疏文件、共享块、压缩、快照及已删除但仍打开的文件会使结果与磁盘总使用量不同。</p>
  </div>;
}

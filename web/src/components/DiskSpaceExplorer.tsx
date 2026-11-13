import {SensitiveScanDialog} from "./SensitiveScanDialog";
import {sensitiveScanPath} from "../lib/scanSafety";
import {ScanProgress} from "./ScanProgress";
import {ScanBackNavigation,useScanBackView} from "./ScanBackNavigation";
import {readSpaceSession,rememberSpaceNode,readMachineSession,rememberMachineSession,touchSpaceSession,readSpaceView,rememberSpaceView} from "../lib/diskSpaceSession";
import {isFullPrivilege,isWindows} from "../lib/hostPlatform";
import {useCallback,useEffect,useId,useMemo,useRef,useState} from "react";
import {pauseSpaceScan,fetchSpaceHomes,keepSpaceResults,releaseSpaceResults,type SpaceStorage,cancelSpaceScan,fetchSpaceScan,fetchSpaceStatus,startSpaceScan,type SpaceEntry,type SpaceResult,type SpaceScan,type SpaceStatus} from "../api/diskSpace";
import {spaceEntryColor,entryPath,parentPercent,sortedEntries,spaceTreemap,type SpaceBasis,type SpaceSort} from "../lib/diskSpace";
import {formatBytes} from "../lib/format";
import {capacityKnown,directoryScanMounts,isBootMount,scanDiskTargets} from "../lib/storageMounts";
import type {NodeSummary} from "../types/monitoring";
import {PagedSpaceVisualization,ParentDirectoryButton} from "./PagedSpaceVisualization";
import "./DiskSpaceExplorer.css";

const CUSTOM_PATH="__custom__";

const statusText:Record<string,string>={running:"扫描中",complete:"扫描完成",partial:"部分结果（未完整扫描）",cancelled:"已取消（部分结果）",failed:"扫描失败",interrupted:"平台重启，扫描中断"};
const issueText:Record<string,string>={permission_denied:"无权读取",changed_or_denied:"条目变化或无法读取",unrepresentable_name:"文件名编码不支持",path_limit:"路径长度达到限制",depth_limit:"达到层级限制",hardlink:"硬链接，已在别处计数",symlink:"符号链接，不跟随",mount_boundary:"其他挂载点，未扫描",special:"特殊文件，不扫描内容"};
const reasonText:Record<string,string>={time_limit:"达到所选扫描时限",entry_limit:"达到所选条目上限",cancelled:"手动取消",unreadable_entries:"部分目录无权读取或层级受限",platform_storage_low:"平台结果存储空间不足，已停止扫描以保护数据库",platform_storage_unknown:"无法确认平台存储可用空间，已安全停止扫描",memory_limit:"扫描内存达到保护上限",platform_memory_low:"平台可用内存不足，已停止继续扫描；已有内存结果仍可查看"};

export function DiskSpaceExplorer({nodes,showBoot=true}:{nodes:NodeSummary[];showBoot?:boolean}){
  const [selected,setSelected]=useState(()=>readSpaceSession()?.node??"");
  const node=nodes.find(n=>n.id===selected)??nodes[0];
  useEffect(()=>{if(node)rememberSpaceNode(node.id);},[node?.id]);
  return <ScanBackNavigation><article className="data-panel disk-explorer">
    <header className="panel-header"><div><h2>磁盘空间分析</h2><p>按目录层级定位空间占用 · 手动扫描 · 只读 · 全量快照持久保存，实时扫描边扫边看</p></div>
      <label className="disk-explorer__machine">选择机器<select value={node?.id??""} onChange={e=>setSelected(e.target.value)} aria-label="磁盘空间分析机器">
        {nodes.map(n=><option key={n.id} value={n.id}>{n.hostname}{n.online?"":" · 离线"}</option>)}
      </select></label></header>
    {node?<MachineSpace key={node.id} node={node} showBoot={showBoot}/>:<p className="table-empty">暂无机器。</p>}
  </article></ScanBackNavigation>;
}

function MachineSpace({node,showBoot}:{node:NodeSummary;showBoot:boolean}){
  const [saved]=useState(()=>readMachineSession(node.id));
  const [storage,setStorage]=useState<SpaceStorage>(saved?.storage??"realtime");
  const [viewer]=useState(()=>crypto.randomUUID());
  useEffect(()=>{
    const controller=new AbortController();
    const attach=()=>{touchSpaceSession();void keepSpaceResults(node.id,viewer,controller.signal).catch(()=>{});};
    const detach=()=>{touchSpaceSession();releaseSpaceResults(node.id,viewer);};
    attach();const timer=setInterval(attach,30_000);
    const onVisible=()=>{if(!document.hidden)attach();};
    window.addEventListener("pagehide",detach);window.addEventListener("pageshow",attach);
    document.addEventListener("visibilitychange",onVisible);
    return()=>{controller.abort();clearInterval(timer);window.removeEventListener("pagehide",detach);window.removeEventListener("pageshow",attach);document.removeEventListener("visibilitychange",onVisible);detach();};
  },[node.id,viewer]);
  const windows=isWindows(node);
  const [status,setStatus]=useState<SpaceStatus|null>(null);const [scan,setScan]=useState<SpaceScan|null>(null);
  const [selected,setSelected]=useState(()=>saved?.scans[saved.storage]??"");const [path,setPath]=useState(saved?.path??"");const [customPath,setCustomPath]=useState(saved?.customPath??false);const full=status?.full_access??isFullPrivilege(node.metrics?.privilege);
  const restoredTask=useRef(selected);const [resumeNotice,setResumeNotice]=useState("");
  const mode=full?"full":"basic";
  const [scope,setScope]=useState<"directory"|"disk"|"all_local">(saved?.scope??"directory");
  const [diskKey,setDiskKey]=useState(saved?.diskKey??"");
  const diskTargets=useMemo(()=>scanDiskTargets(node.metrics?.disks??[],node.metrics?.block_devices??[],showBoot),[node.metrics?.disks,node.metrics?.block_devices,showBoot]);
  const selectedDisk=diskTargets.find(target=>target.key===diskKey);
  const scanPath=scope==="disk"?selectedDisk?.disk.mount_point??"":scope==="all_local"?"/":path;
  const [seconds,setSeconds]=useState(saved?.seconds??0);const [maxEntries,setMaxEntries]=useState(saved?.maxEntries??0);const [busy,setBusy]=useState(false);const [error,setError]=useState("");
  const [refresh,setRefresh]=useState(0);
  const [sensitiveRequest,setSensitiveRequest]=useState<import("../api/diskSpace").SpaceScanRequest>();
  useEffect(()=>{
    const scans={...(readMachineSession(node.id)?.scans??{}),[storage]:selected};
    rememberMachineSession(node.id,{storage,scans,path,customPath,scope,diskKey,seconds,maxEntries});
  },[node.id,storage,selected,path,customPath,scope,diskKey,seconds,maxEntries]);
  const loaded=useRef("");
  const [homes,setHomes]=useState<string[]>([]);const [homesError,setHomesError]=useState("");
  const homePaths=[...new Set(homes)].sort((a,b)=>a.localeCompare(b));
  const homeSet=new Set(homePaths);
  const commonPaths=[...new Set([status?.default_path,...directoryScanMounts(node.metrics?.disks??[]).map(d=>d.mount_point)])]
    .filter((p):p is string=>!!p&&!homeSet.has(p)&&(showBoot||!isBootMount(p)));
  const presetPaths=[...homePaths,...commonPaths];
  const directoryChoice=customPath?CUSTOM_PATH:path;
  const choosePath=(value:string)=>{setPath(value);setCustomPath(!presetPaths.includes(value));setScope("directory");};
  useEffect(()=>{
    if(windows)return;
    const controller=new AbortController();let timer:ReturnType<typeof setTimeout>;
    const load=async()=>{try{
      const result=await fetchSpaceHomes(node.id,controller.signal);
      if(!controller.signal.aborted){setHomes(result.paths);setHomesError("");}
    }catch(e){if(!controller.signal.aborted)setHomesError(e instanceof Error?e.message:"读取 /home 目录失败");}
    if(!controller.signal.aborted)timer=setTimeout(load,300_000);};
    void load();return()=>{controller.abort();clearTimeout(timer);};
  },[node.id,windows]);
  useEffect(()=>{
    const controller=new AbortController();let timer:ReturnType<typeof setTimeout>;
    async function load(){
      if(controller.signal.aborted)return;
      if(document.hidden){timer=setTimeout(load,60000);return;}
      try{
        const next=await fetchSpaceStatus(node.id,controller.signal,storage);if(controller.signal.aborted)return;
        if(restoredTask.current){
          if(!next.scans.some(s=>s.id===restoredTask.current))setResumeNotice("上次选择的扫描结果已释放或不可用；可选择当前记录或重新扫描。");
          restoredTask.current="";
        }
        setStatus(next);setError("");
        const id=next.scans.some(s=>s.id===selected)?selected:next.scans[0]?.id;
        const summary=next.scans.find(s=>s.id===id);
        if(summary?.status==="running"&&storage==="snapshot"){setScan(summary);loaded.current="";}
        else if(summary){
          const key=`${summary.id}:${summary.status}:${summary.finished_at}:${summary.revision??0}:${summary.expires_at??""}:${summary.paused}:${summary.pause_pending}:${summary.pause_supported}`;
          if(loaded.current!==key){const detail=await fetchSpaceScan(node.id,summary.id,controller.signal);if(!controller.signal.aborted){setScan(detail);loaded.current=key;}}
        }else{setScan(null);loaded.current="";}
        if(!controller.signal.aborted&&id&&id!==selected)setSelected(id);
        if(!controller.signal.aborted)timer=setTimeout(load,((next.active_count??0)>0||next.active_scan||next.scans.some(s=>s.status==="running"))?(storage==="realtime"?1000:3000):60000);
      }catch(e){if(!controller.signal.aborted){setError(e instanceof Error?e.message:"读取扫描记录失败");timer=setTimeout(load,storage==="realtime"?3000:60000);}}
    }
    void load();return()=>{controller.abort();clearTimeout(timer);};
  },[node.id,selected,refresh,storage]);
  useScanBackView(()=>!status&&!error?"pending":scan?.result?"pending":"root",false,true);
  const running=status?.active_scan??status?.scans.some(s=>s.status==="running");
  const activeCount=status?.active_count??(running?1:0);
  const atCapacity=activeCount>=(status?.limits.concurrent??10);
  const canCancel=scan?.mode!=="full"||status?.root_authorized===true;
  async function start(){
    if(scope==="disk"&&!selectedDisk){setError("请选择当前机器的已挂载本地磁盘");return;}
    if(sensitiveScanPath(scanPath,scope==="all_local"?"all_local":"directory",windows)){
      setSensitiveRequest({path:scanPath,mode,seconds,max_entries:maxEntries,scope:scope==="all_local"?"all_local":"directory",storage,viewer});return;
    }
    setBusy(true);setError("");
    try{
      // A disk choice is a scan rooted at its mounted filesystem; the server
      // revalidates the path and mount boundaries using the existing scanner.
      const result=await startSpaceScan(node.id,scanPath,mode,seconds,maxEntries,scope==="all_local"?"all_local":"directory",storage,viewer);
      setSelected(result.id);setRefresh(v=>v+1);void keepSpaceResults(node.id,viewer).catch(()=>{});
    }catch(e){setError(e instanceof Error?e.message:"扫描启动失败");}finally{setBusy(false);}
  }
  async function pause(){if(!scan)return;setBusy(true);setError("");try{await pauseSpaceScan(node.id,scan.id,!scan.paused);setRefresh(v=>v+1);}catch(e){setError(e instanceof Error?e.message:"暂停/继续失败");}finally{setBusy(false);}}
  async function cancel(){if(!scan)return;setBusy(true);try{await cancelSpaceScan(node.id,scan.id);setRefresh(v=>v+1);}catch(e){setError(e instanceof Error?e.message:"取消失败");}finally{setBusy(false);}}
  return <div className="disk-explorer__body">
    {sensitiveRequest&&<SensitiveScanDialog nodeId={node.id} hostname={node.hostname} platform={windows?"windows":"linux"} request={sensitiveRequest} onClose={()=>setSensitiveRequest(undefined)} onStarted={id=>{setSensitiveRequest(undefined);setSelected(id);setRefresh(v=>v+1);void keepSpaceResults(node.id,viewer).catch(()=>{});}}/>}
    <div className="disk-explorer__modes" role="group" aria-label="磁盘空间扫描模式">
      {([['realtime','实时扫描模式'],['snapshot','全量快照模式']] as const).map(([value,label])=><button type="button" key={value}
        className={`disk-explorer__mode ${storage===value?'disk-explorer__mode--active':''}`} aria-pressed={storage===value} disabled={busy}
        onClick={()=>{if(storage!==value){setStorage(value);setSelected(readMachineSession(node.id)?.scans[value]??"");setScan(null);setStatus(null);setError("");loaded.current="";}}}>{label}</button>)}
    </div>
    <p className="hint disk-explorer__mode-help">{storage==="snapshot"?"扫描完成后保存目录快照，供后续查看。":"点击即开始扫描，目录树与空间占用边扫描边更新。可同时扫描不同位置，保留多份独立的内存结果。"}</p>
    <div className="disk-explorer__controls">
      <label>扫描范围<select value={scope} onChange={e=>setScope(e.target.value as typeof scope)} disabled={busy}>
        <option value="directory">指定目录</option><option value="disk">指定磁盘</option><option value="all_local">全部本地磁盘</option>
      </select></label>
      <label>扫描级别<select className="disk-explorer__fixed-level" value={mode} disabled aria-label="扫描级别（由 Agent 权限决定）">
        <option value="basic">普通用户 · {status?.username??"原 SSH 账户"}</option><option value="full">{windows?"Windows 管理员":"root"} · Full Agent</option>
      </select></label>
      {scope==="disk"?<label className="disk-explorer__path">选择磁盘 / 分区<select value={selectedDisk?.key??""} onChange={e=>setDiskKey(e.target.value)} disabled={busy||!diskTargets.length} aria-label="选择要扫描的本地磁盘">
        <option value="">{diskTargets.length?"请选择磁盘 / 分区":"暂无可扫描的已挂载本地磁盘"}</option>
        {diskTargets.map(target=><option key={target.key} value={target.key}>{target.disk.name} · {target.disk.mount_point} · {capacityKnown(target.disk)?formatBytes(target.disk.total_bytes):"容量未知"}{target.subtree?" · 子目录挂载":""}</option>)}
      </select></label>:scope==="all_local"?<label className="disk-explorer__path">目录路径<input value="自动发现已挂载的本地文件系统" disabled/></label>:<div className="disk-explorer__path disk-explorer__directory">
        <label>目录路径<select aria-label="选择扫描目录" value={directoryChoice} disabled={busy} onChange={e=>{const value=e.target.value;setCustomPath(value===CUSTOM_PATH);setPath(value===CUSTOM_PATH?"":value);}}>
          <option value="">请下拉选择扫描目录</option>
          {!customPath&&path&&!presetPaths.includes(path)&&<option value={path}>{path} · 上次选择</option>}
          {homePaths.length>0&&<optgroup label="/home 子目录">{homePaths.map(p=><option key={p} value={p}>{p}</option>)}</optgroup>}
          {commonPaths.length>0&&<optgroup label="常用目录 / bigdata 数据目录">{commonPaths.map(p=><option key={p} value={p}>{p}</option>)}</optgroup>}
          <option value={CUSTOM_PATH}>自定义路径…</option>
        </select></label>
        {directoryChoice===CUSTOM_PATH&&<label>自定义路径<input autoFocus value={path} disabled={busy} onChange={e=>setPath(e.target.value)} placeholder={windows?"请输入 C:/... 绝对路径":"请输入 /... 绝对路径"} maxLength={2048} spellCheck={false} aria-label="扫描目录绝对路径"/></label>}
      </div>}
      <label>扫描时限<select value={seconds} onChange={e=>setSeconds(Number(e.target.value))}><option value={0}>无限制</option><option value={30}>30 秒</option><option value={60}>60 秒</option><option value={120}>120 秒</option></select></label>
      <label>条目数量<select value={maxEntries} onChange={e=>setMaxEntries(Number(e.target.value))}><option value={0}>无限制</option><option value={20000}>20,000 项</option><option value={100000}>100,000 项</option></select></label>
      <button className="button" disabled={!node.online||!status||busy||atCapacity||!(windows?/^[A-Za-z]:[\\/]/.test(scanPath)||scope==="all_local":scanPath.startsWith("/"))} onClick={()=>void start()}>{busy?"处理中…":atCapacity?"扫描名额已满":activeCount>0?"开始另一个扫描":"开始扫描"}</button>
    </div>
    {!windows&&scope==="directory"&&homesError&&<p className="hint">/home 目录备选读取失败：{homesError}。仍可选择常用目录或自定义路径。</p>}
    {scope==="disk"&&selectedDisk&&<p className="hint disk-explorer__disk-selection">扫描挂载点：<strong>{selectedDisk.disk.mount_point}</strong> · {selectedDisk.disk.file_system}{selectedDisk.aliases>0&&<> · 已合并 {selectedDisk.aliases} 个重复共享入口</>}{selectedDisk.subtree&&" · 当前仅挂载了文件系统的子目录，扫描只覆盖这个子目录"}</p>}
    {scope==="disk"&&diskKey&&!selectedDisk&&<p className="disk-explorer__notice">原选磁盘已不在当前可选清单中，请重新选择。</p>}
    {status&&<p className="hint" role="status">平台并行扫描 {activeCount} / {status.limits.concurrent} · 内存结果 {status.memory_result_count??status.scans.length} / {status.limits.memory_results??100}。达到 100 份时自动清理最旧的已结束结果，运行中和暂停中的任务保留。</p>}
    <p className="hint">时间与条目数量默认无限制，直到扫描完成或手动取消；仅汇总目录大小、数量和层级，不保留文件名及逐文件明细。保持低优先级、只读，同一机器可并行扫描，平台最多同时 10 个任务（含暂停任务）；每个文件系统最大 64 层，不跟随符号链接。关闭或 F5 刷新页面后任务继续；本浏览器记住选择与查看位置 24 小时，回来可继续上次扫描。</p>
    {storage==="snapshot"?<p className="hint">平台结果存储分区可用空间低于 2 GiB 时，会停止扫描并保存不完整快照；释放空间或迁移数据库后可重新扫描。</p>:<p className="hint">实时结果仅驻留平台内存，不写入数据库或结果文件。页面打开期间持续保留；最后一个查看页面关闭或离开后保留 24 小时，到期自动释放；再次打开重新续期。平台服务重启会清空实时结果。可用内存低于 512 MiB 时停止继续扫描，已有结果仍可查看。</p>}
    <p className="hint">已启用 Full Agent 的机器固定使用 root（Windows 使用管理员）进行只读扫描，普通目录向所有访问者开放；顶层和系统目录需管理员验证、安全检查和再次确认后才能启动。Basic 机器按原 SSH 用户权限扫描。两种模式的结果均按机器共享。</p>
    <p className={scope==="all_local"?"disk-explorer__notice":"hint"}>{scope==="all_local"?"一次扫描所有已挂载且支持的本地文件系统，按磁盘分别展示并汇总；绑定挂载去重，网络盘、虚拟文件系统和未挂载分区不扫描。普通用户无权读取的部分会标为不完整。全机扫描可能持续较久，并增加 I/O 和结果存储占用。":scope==="disk"?"指定磁盘按已挂载的分区或文件系统选择，多分区磁盘分别列出。仅扫描所选挂载点，不跨其他挂载点；网络盘、虚拟文件系统和未挂载分区不列入。启动分区跟随页面显隐设置。":"指定目录仅扫描所在文件系统，跳过其他挂载点；要一次查看各块本地磁盘，请选择「全部本地磁盘」。"}</p>
    {mode==="basic"&&<p className="hint">以 {status?.username??"原 SSH 用户"} 的实际访问权限扫描；受限目录标为未完整读取。普通用户扫描结果遵循平台现有内网监控页面的访问范围。</p>}
    {!node.online&&<p className="disk-explorer__notice">机器离线，仍可查看保留的扫描结果；上线后可重新扫描。</p>}
    {error&&<p className="form-error" role="alert">{error}</p>}
    {resumeNotice&&<p className="disk-explorer__notice" role="status">{resumeNotice}</p>}
    <div className="disk-explorer__history"><label>{storage==="snapshot"?"扫描快照":"实时扫描结果"}<select value={scan?.id??""} onChange={e=>setSelected(e.target.value)} disabled={!status?.scans.length}>
      {!status?.scans.length&&<option value="">{storage==="snapshot"?"尚无扫描快照":"尚无实时扫描结果"}</option>}
      {status?.scans.map(s=><option key={s.id} value={s.id}>{new Date(s.started_at).toLocaleString()} · {s.mode==="full"?"root":"普通用户"} · {s.scope==="all_local"?"全部本地磁盘":s.root_path} · {statusText[s.status]??s.status}</option>)}
    </select></label><span className="hint">{storage==="snapshot"?"每级别保留最近 3 次，最长 7 天":`${status?.scans.length??0} 份内存结果 · 关闭页面后保留 24 小时`}</span></div>
    {running&&!status?.scans.some(s=>s.status==="running")&&<p className="hint">该机器正在另一模式中扫描，仍可开始新任务。</p>}
    {scan&&<div className="disk-explorer__scan-state" data-status={scan.status} role="status"><strong>{scan.paused?"已暂停":statusText[scan.status]??scan.status}</strong>
      <span className="disk-explorer__progress-count">{scan.result?.visited??scan.progress.visited??0} 个条目 · {scan.result?.elapsed_seconds??scan.progress.elapsed_seconds??0} 秒</span>
      <ScanProgress key={scan.id} scan={scan}/>
      {scan.status==="running"&&<>{scan.pause_supported&&<button className="button button--secondary button--small" disabled={busy||scan.pause_pending} onClick={()=>void pause()}>{scan.pause_pending?(scan.paused?"继续中…":"暂停中…"):scan.paused?"继续扫描":"暂停扫描"}</button>}<button className="button button--secondary button--small" disabled={busy||!canCancel} title={canCancel?undefined:"当前机器需在线并启用 Full Agent"} onClick={()=>void cancel()}>取消扫描</button></>}
      {scan.message&&<span className="form-error">{scan.message}</span>}
    </div>}
    {scan?.storage==="realtime"&&scan.expires_at&&<p className="hint">无人查看时释放时间：{new Date(scan.expires_at).toLocaleString()}</p>}
    {scan?.result?(scan.result.paged?<PagedSpaceVisualization key={scan.id} nodeId={node.id} hostname={node.hostname} scanId={scan.id} result={{...scan.result,paused:scan.paused}} onChoosePath={choosePath}/>:<SpaceVisualization key={scan.id} nodeId={node.id} scanId={scan.id} result={{...scan.result,paused:scan.paused}} onChoosePath={choosePath}/>):scan?.status==="running"&&storage==="realtime"?<p className="table-empty" role="status">正在连接扫描器，目录结果将陆续显示…</p>:scan?.status!=="running"&&<p className="table-empty">{scan&&["failed","interrupted"].includes(scan.status)?"本次未生成完整汇总；已写入的条目保留，无法作为完整扫描结果展示。请解决提示的问题后重新扫描。":"选择目录并开始扫描，即可按层级查看空间占用。"}</p>}
  </div>;
}

export function SpaceVisualization({result,onChoosePath,nodeId,scanId}:{result:SpaceResult;onChoosePath:(path:string)=>void;nodeId?:string;scanId?:string}){
  const [saved]=useState(()=>nodeId&&scanId?readSpaceView(nodeId,scanId):undefined);
  const [directory,setDirectory]=useState(saved?.directory??0);const [basis,setBasis]=useState<SpaceBasis>(saved?.basis??"allocated");
  const [treemapCollapsed,setTreemapCollapsed]=useState(saved?.treemapCollapsed===true);
  const [sort,setSort]=useState<SpaceSort>(saved?.sort??"size");const [ascending,setAscending]=useState(saved?.ascending??false);
  const [expanded,setExpanded]=useState<Set<number>>(()=>new Set(saved?.expanded??[]));const [search,setSearch]=useState(saved?.search??"");const [limit,setLimit]=useState(250+(saved?.offset??0));
  useEffect(()=>{if(nodeId&&scanId)rememberSpaceView(nodeId,scanId,{directory,basis,sort,ascending,search,offset:Math.max(0,limit-250),treemapCollapsed,expanded:[...expanded].slice(0,8)});},[nodeId,scanId,directory,basis,sort,ascending,search,limit,expanded,treemapCollapsed]);
  const clipPrefix=useId().replace(/:/g,"");
  const nodes=result.nodes;const current=nodes[directory]??nodes[0];
  const children=useMemo(()=>{const map=new Map<number,SpaceEntry[]>();for(const n of nodes){if(n.parent!==null){const list=map.get(n.parent)??[];list.push(n);map.set(n.parent,list);}}return map;},[nodes]);
  const cursor=useRef(directory);
  const navigate=useCallback((id:number)=>{if(nodes[id]?.kind!=="directory")return;cursor.current=id;setDirectory(id);setSearch("");setExpanded(new Set());setLimit(250);},[nodes]);
  const back=useScanBackView(()=>{const parent=nodes[cursor.current]?.parent;if(parent==null)return "root";navigate(parent);return "up";});
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
  const rectangles=treemapCollapsed?[]:spaceTreemap(shown);
  function sortBy(key:SpaceSort){if(sort===key)setAscending(v=>!v);else{setSort(key);setAscending(key==="name");}setLimit(250);}
  const sortButton=(key:SpaceSort,label:string)=><button className="disk-explorer__sort" onClick={()=>sortBy(key)}>{label} {sort===key?(ascending?"↑":"↓"):"↕"}</button>;
  return <div className="disk-explorer__visual" {...back.props}>
    {result.status!=="complete"&&<p className="disk-explorer__notice">{result.paused?"扫描已暂停，已有结果可继续查看":reasonText[result.reason]??"本次扫描未完成"}。大小为已读取部分的下限；百分比仅相对于已扫描数据，不代表完整目录。可选择更小的目录再次扫描。</p>}
    <div className="disk-explorer__summary"><div><span>实际占用</span><strong>{formatBytes(current.allocated)}{current.incomplete?" +":""}</strong></div><div><span>文件大小</span><strong>{formatBytes(current.apparent)}{current.incomplete?" +":""}</strong></div><div><span>文件 / 子目录</span><strong>{current.files.toLocaleString()} / {current.directories.toLocaleString()}</strong></div><div><span>扫描身份</span><strong>{result.username} · {result.native_id?`SID ${result.native_id}`:`UID ${result.uid}`}</strong></div></div>
    <nav className="disk-explorer__breadcrumbs" aria-label="目录层级">{crumbs.map((e,i)=><span key={e.id}>{i>0&&<span aria-hidden="true"> / </span>}<button disabled={e.id===current.id} onClick={()=>navigate(e.id)}>{e.name}</button></span>)}</nav>
    <div className="disk-explorer__toolbar"><label>统计口径<select value={basis} onChange={e=>setBasis(e.target.value as SpaceBasis)}><option value="allocated">实际磁盘占用</option><option value="apparent">文件大小（逻辑大小）</option></select></label>
      <button className="button button--secondary button--small" onClick={()=>{setExpanded(new Set());setLimit(250);}}>折叠全部</button>
      <button className="button button--secondary button--small" onClick={()=>{setExpanded(new Set(direct.filter(e=>e.kind==="directory").map(e=>e.id)));setLimit(250);}}>展开下一层</button>
      <button className="button button--secondary button--small" onClick={()=>onChoosePath(entryPath(current,nodes))}>将此目录设为扫描范围</button>
      <button type="button" className="button button--secondary button--small disk-explorer__treemap-toggle" aria-expanded={!treemapCollapsed} aria-controls={clipPrefix+"-treemap"} onClick={()=>setTreemapCollapsed(v=>!v)}>{treemapCollapsed?"展开本图":"折叠本图"}</button>
    </div>
    <div id={clipPrefix+"-treemap"} hidden={treemapCollapsed}>{rectangles.length>0?<svg className="disk-explorer__treemap" viewBox="0 0 1000 300" aria-label="当前目录空间占用矩形图，面积代表已扫描大小">
      {rectangles.map((rect,i)=>{const entry=nodes[rect.id];const navigable=entry?.kind==="directory"&&entry.issue!=="mount_boundary";const label=`${rect.name} · ${formatBytes(rect.value)} · ${current[basis]>0?(rect.value/current[basis]*100).toFixed(1):0}%`;
        return <g key={rect.id} role={navigable?"button":undefined} tabIndex={navigable?0:undefined} aria-label={label} onClick={()=>navigable&&navigate(rect.id)} onKeyDown={e=>{if(navigable&&(e.key==="Enter"||e.key===" ")){e.preventDefault();navigate(rect.id);}}}>
          <title>{label}</title><defs><clipPath id={`${clipPrefix}-${i}`}><rect x={rect.x+5} y={rect.y+3} width={Math.max(0,rect.width-10)} height={Math.max(0,rect.height-6)}/></clipPath></defs>
          <rect x={rect.x} y={rect.y} width={rect.width} height={rect.height} fill={`var(--space-color-${i%6})`} stroke="var(--surface)" strokeWidth={2}/>
          {rect.width>60&&rect.height>28&&<g clipPath={`url(#${clipPrefix}-${i})`}><text x={rect.x+9} y={rect.y+20}>{rect.name}</text>{rect.height>48&&<text x={rect.x+9} y={rect.y+40} className="disk-explorer__treemap-size">{formatBytes(rect.value)}</text>}</g>}
        </g>;})}
    </svg>:<p className="hint">当前目录没有已读取且大于 0 的占用，或读取范围受限。</p>}</div>
    <div className="disk-explorer__filter"><label>筛选已展开项<input value={search} onChange={e=>{setSearch(e.target.value);setLimit(250);}} placeholder="名称关键字"/></label><span className="hint">单击目录名展开或收起，双击进入；箭头也可展开层级。矩形图展示前 40 项，其余合并。</span></div>
    <div className="table-scroll"><table className="disk-explorer__tree"><thead><tr>
      <th><div className="disk-explorer__name-heading">{sortButton("name","名称 / 层级")}<ParentDirectoryButton parent={current.parent} onNavigate={navigate}/></div></th><th>{sortButton("size",basis==="allocated"?"实际占用":"文件大小")}</th><th>{sortButton("percent","占上一层")}</th><th>{sortButton("files","文件数")}</th><th>子目录</th><th>状态</th>
    </tr></thead><tbody>{rows.slice(0,limit).map(({entry,depth})=>{const percent=parentPercent(entry,nodes,basis);const hasChildren=children.has(entry.id);const unknown=entry.incomplete&&entry.files===0&&entry.directories===0;return <tr key={entry.id}>
      <td><div className="disk-explorer__entry disk-explorer__entry--group" style={{color:spaceEntryColor(entry,"label"),paddingLeft:`${Math.min(depth,16)*16}px`}}>
        <button className="disk-explorer__expand" disabled={!hasChildren} aria-label={`${expanded.has(entry.id)?"折叠":"展开"} ${entry.name}`} aria-expanded={hasChildren?expanded.has(entry.id):undefined} onClick={()=>setExpanded(previous=>{const next=new Set(previous);if(next.has(entry.id))next.delete(entry.id);else next.add(entry.id);return next;})}>{hasChildren?(expanded.has(entry.id)?"▾":"▸"):"·"}</button>
        <button className="disk-explorer__name" title={(hasChildren?"单击展开或收起，双击进入目录":"双击进入目录")+" · "+entryPath(entry,nodes)} disabled={entry.kind!=="directory"||entry.issue==="mount_boundary"} aria-expanded={hasChildren?expanded.has(entry.id):undefined} onClick={e=>{if(e.detail<2&&hasChildren)setExpanded(previous=>{const next=new Set(previous);if(next.has(entry.id))next.delete(entry.id);else next.add(entry.id);return next;});}} onDoubleClick={()=>{navigate(entry.id);}} onKeyDown={e=>{if(!e.repeat&&(e.key==="Enter"||e.key===" ")){e.preventDefault();if(e.key==="Enter")navigate(entry.id);else if(hasChildren)setExpanded(previous=>{const next=new Set(previous);if(next.has(entry.id))next.delete(entry.id);else next.add(entry.id);return next;});}}}><span aria-hidden="true">{entry.kind==="directory"?"▰":"▱"} </span>{entry.name}</button>
      </div></td><td className="number">{unknown?"未完整读取":formatBytes(entry[basis])}{entry.incomplete&&!unknown?" +":""}</td>
      <td><div className="disk-explorer__percent"><span>{unknown||percent===null?"—":`${percent.toFixed(1)}%`}</span><div><i style={{backgroundColor:spaceEntryColor(entry),width:`${unknown?0:percent??0}%`}}/></div></div></td>
      <td className="number">{entry.files.toLocaleString()}{entry.incomplete?" +":""}</td><td className="number">{entry.directories.toLocaleString()}{entry.incomplete?" +":""}</td>
      <td className={entry.incomplete?"disk-explorer__warning":"hint"}>{entry.issue?(issueText[entry.issue]??entry.issue):entry.incomplete?"下限值":"已读取"}</td>
    </tr>;})}</tbody></table></div>
    {!rows.length&&<p className="table-empty">当前层级没有匹配条目。</p>}{rows.length>limit&&<button className="button button--secondary button--small" onClick={()=>setLimit(n=>n+250)}>继续显示 {Math.min(250,rows.length-limit)} 项（共 {rows.length.toLocaleString()} 项）</button>}
    <p className="hint">{result.filesystem} · {result.errors} 个读取问题 · 跳过 {result.skipped_mounts} 个挂载点 · {result.symlinks} 个符号链接 · {result.hardlinks} 个重复硬链接。硬链接仅首次计入，目录分摊受遍历顺序影响。实际占用来自文件系统分配块；稀疏文件、共享块、压缩、快照及已删除但仍打开的文件会使结果与磁盘总使用量不同。</p>
  </div>;
}

import {SensitiveScanDialog} from "./SensitiveScanDialog";
import {sensitiveScanPath} from "../lib/scanSafety";
import {useScanBackView} from "./ScanBackNavigation";
import {ManualSpaceScan} from "./ManualSpaceScan";
import {readLinkedScans,rememberLinkedScan,readSpaceView,rememberSpaceView} from "../lib/diskSpaceSession";
import {Fragment,useCallback,useEffect,useId,useRef,useState,type ReactNode} from "react";
import {fetchSpaceStatus,startSpaceScan,fetchSpacePage,type SpacePage,type SpaceResult,type SpaceScan} from "../api/diskSpace";
import {spaceEntryColor,spaceTreemap,spaceDirectoryPath,excludedSpaceEntry,directorySummaryItems,stableSpacePage,type SpaceBasis,type SpaceSort} from "../lib/diskSpace";
import {formatBytes} from "../lib/format";

const issues:Record<string,string>={permission_denied:"无权读取",changed_or_denied:"条目变化或无法读取",unrepresentable_name:"文件名编码不支持",path_limit:"路径长度达到限制",depth_limit:"达到层级限制",hardlink:"硬链接，已在别处计数",symlink:"符号链接，不跟随",mount_boundary:"挂载点，未扫描",duplicate_mount:"绑定挂载，在其他位置统计",separate_filesystem:"独立文件系统，见磁盘总览",mount_changed:"挂载发生变化，请重新扫描",no_local_filesystems:"没有可扫描的本地文件系统",special:"特殊文件"};
const reasons:Record<string,string>={time_limit:"达到所选时限",entry_limit:"达到所选条目上限",cancelled:"手动取消",unreadable_entries:"部分目录读取受限",platform_storage_low:"平台结果存储空间不足，已停止扫描以保护数据库",platform_storage_unknown:"无法确认平台存储可用空间，已安全停止扫描",memory_limit:"扫描内存达到保护上限",scanning:"实时扫描中，目录大小持续更新",connection_lost:"扫描已中断，显示已读取的目录",platform_memory_low:"平台可用内存不足，已停止继续扫描"};
const directoryPath=spaceDirectoryPath;

export function ParentDirectoryButton({parent,onNavigate,disabled=false,onRootBack}:{parent:number|null;onNavigate:(id:number)=>void;disabled?:boolean;onRootBack?:()=>void}){
  return <button type="button" className="disk-explorer__up" aria-label="返回上一级目录" title={parent===null&&!onRootBack?"已在扫描根目录":"返回上一级目录"} disabled={(parent===null&&!onRootBack)||disabled} onClick={()=>{if(parent!==null)onNavigate(parent);else onRootBack?.();}}>
    <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round"><path d="M12 20V4m-7 7 7-7 7 7"/></svg>
  </button>;
}

export function PagedSpaceVisualization({nodeId,scanId,result,onChoosePath,compact=false,onRootBack,hostname=nodeId}:{nodeId:string;hostname?:string;scanId:string;result:SpaceResult;onChoosePath:(path:string)=>void;compact?:boolean;onRootBack?:()=>void}){
  const [saved]=useState(()=>readSpaceView(nodeId,scanId));
  const [directory,setDirectory]=useState(saved?.directory??0);const [basis,setBasis]=useState<SpaceBasis>(saved?.basis??"allocated");
  const [treemapCollapsed,setTreemapCollapsed]=useState(saved?.treemapCollapsed===true);
  const [sort,setSort]=useState<SpaceSort>(saved?.sort??"size");const [ascending,setAscending]=useState(saved?.ascending??false);
  const [search,setSearch]=useState(saved?.search??"");const [filter,setFilter]=useState(saved?.search??"");
  const [pages,setPages]=useState<Map<number,SpacePage>>(new Map());const [expanded,setExpanded]=useState<Set<number>>(()=>new Set(saved?.expanded??[]));
  const [manualOpen,setManualOpen]=useState<Set<number>>(()=>new Set(saved?.linked??[]));
  const [manualJobs,setManualJobs]=useState<Map<number,{id?:string;path:string;starting?:boolean;error?:string}>>(()=>new Map(Object.entries(readLinkedScans(nodeId,scanId)).map(([id,record])=>[Number(id),record])));
  const [manualDetails,setManualDetails]=useState<Map<number,SpaceScan>>(new Map());
  const manualPending=useRef(new Set<number>());
  const [sensitiveManual,setSensitiveManual]=useState<{entry:number;request:import("../api/diskSpace").SpaceScanRequest}>();
  async function startManual(id:number,path:string){
    if(manualPending.current.has(id))return;
    if(sensitiveScanPath(path,"directory",result.platform==="windows")){
      setSensitiveManual({entry:id,request:{path,mode:result.uid===0||result.administrator===true?"full":"basic",seconds:0,max_entries:0,scope:"directory",storage:"realtime"}});return;
    }
    manualPending.current.add(id);
    setManualJobs(old=>new Map(old).set(id,{...old.get(id),path,starting:true}));
    try{
      const state=await fetchSpaceStatus(nodeId,undefined,"realtime");
      const existing=state.scans.find(s=>s.status==="running"&&s.root_path===path&&s.id!==scanId);
      if(!existing&&(state.active_count??0)>=(state.limits?.concurrent??10))throw new Error("平台同时扫描已达 10 个（包含暂停任务），请等待或取消现有任务。");
      const job=existing??await startSpaceScan(nodeId,path,result.uid===0||result.administrator===true?"full":"basic",0,0,"directory","realtime");
      rememberLinkedScan(nodeId,scanId,id,{id:job.id,path});
      setManualJobs(old=>new Map(old).set(id,{id:job.id,path}));
    }catch(e){setManualJobs(old=>new Map(old).set(id,{...old.get(id),path,starting:false,error:e instanceof Error?e.message:"目标扫描启动失败"}));}
    finally{manualPending.current.delete(id);}
  }
  function toggleManual(id:number,path:string){
    if(manualOpen.has(id)){setManualOpen(old=>{const next=new Set(old);next.delete(id);return next;});return;}
    setManualOpen(old=>{const next=new Set(old);if(next.size>=8)next.delete(next.values().next().value!);next.add(id);return next;});
    if(!manualJobs.get(id)?.id)void startManual(id,path);
  }
  const [displayedPage,setDisplayedPage]=useState<SpacePage>();
  const displayedDirectory=useRef<number|undefined>(directory);
  const restoredOffset=useRef(saved?.offset??0);
  const latestPages=useRef(pages);latestPages.current=pages;
  const latestExpanded=useRef(expanded);latestExpanded.current=expanded;
  const [busy,setBusy]=useState<Set<number>>(new Set());const [error,setError]=useState("");
  const control=useRef<AbortController|null>(null);const sequence=useRef(new Map<number,number>());
  const retained=useRef(new Set<number>());
  const inFlight=useRef(new Map<number,number>());const refreshPending=useRef(new Set<number>());
  const clipPrefix=useId().replace(/:/g,"");
  useEffect(()=>{const timer=setTimeout(()=>setFilter(search),250);return()=>clearTimeout(timer);},[search]);
  const load=useCallback(async(parent:number,offset=0,background=false):Promise<void>=>{
    const controller=control.current;if(!controller)return;
    if(background&&inFlight.current.has(parent)){refreshPending.current.add(parent);return;}
    const serial=(sequence.current.get(parent)??0)+1;sequence.current.set(parent,serial);
    inFlight.current.set(parent,serial);
    if(!background){setBusy(old=>new Set(old).add(parent));setError("");}
    try{
      const page=await fetchSpacePage(nodeId,scanId,parent,basis,sort,ascending,offset,parent===directory?filter:"",controller.signal);
      if(!controller.signal.aborted&&sequence.current.get(parent)===serial&&retained.current.has(parent)){
        const next=background?stableSpacePage(latestPages.current.get(parent),page):page;
        // Keep the committed view until its replacement has arrived. A click,
        // sort or filter must not unmount the table while fetching metadata.
        if(parent===directory){
          if(displayedDirectory.current!==parent){retained.current=new Set([parent]);setExpanded(new Set());}
          displayedDirectory.current=parent;setDisplayedPage(next);
        }
        setPages(old=>new Map([...old].filter(([id])=>retained.current.has(id))).set(parent,next));
      }
    }catch(e){if(!controller.signal.aborted&&sequence.current.get(parent)===serial&&!background)setError(e instanceof Error?e.message:"目录加载失败");}
    finally{
      if(!controller.signal.aborted&&sequence.current.get(parent)===serial){
        if(inFlight.current.get(parent)===serial)inFlight.current.delete(parent);
        if(!background)setBusy(old=>{const next=new Set(old);next.delete(parent);return next;});
        if(refreshPending.current.delete(parent)&&retained.current.has(parent))void load(parent,offset,true);
      }
    }
  },[nodeId,scanId,basis,sort,ascending,directory,filter]);
  useEffect(()=>{
    const controller=new AbortController();control.current=controller;sequence.current=new Map();
    inFlight.current=new Map();refreshPending.current=new Set();
    const children=displayedDirectory.current===directory?[...latestExpanded.current]:[];
    retained.current=new Set([directory,...children]);
    setBusy(new Set());
    for(const id of retained.current)void load(id,id===directory?restoredOffset.current:0);
    restoredOffset.current=0;
    return()=>controller.abort();
  },[load,directory]);
  const appliedRevision=useRef(result.revision);
  useEffect(()=>{
    if(result.revision===undefined||appliedRevision.current===result.revision)return;
    appliedRevision.current=result.revision;
    for(const id of retained.current)void load(id,latestPages.current.get(id)?.offset??0,true);
  },[result.revision,load]);
  const page=displayedPage;const current=page?.directory??result.nodes[0];
  const offset=pages.get(directory)?.offset??(directory===saved?.directory?saved.offset:0);
  useEffect(()=>{rememberSpaceView(nodeId,scanId,{directory,basis,sort,ascending,search,offset,treemapCollapsed,expanded:[...expanded],linked:[...manualOpen]});},
    [nodeId,scanId,directory,basis,sort,ascending,search,offset,expanded,manualOpen,treemapCollapsed]);
  const navigating=!!page&&page.directory.id!==directory;
  const cursor=useRef(directory);
  function navigate(id:number){cursor.current=id;setDirectory(id);setSearch("");setFilter("");}
  const back=useScanBackView(()=>{
    const candidates=[result.nodes[0],...(page?[page.directory,...page.ancestors,...page.entries,...page.top]:[])];
    for(const cached of latestPages.current.values())candidates.push(cached.directory,...cached.ancestors,...cached.entries,...cached.top);
    const entry=candidates.find(entry=>entry?.id===cursor.current);
    if(!entry){return error?"root":"pending";}
    if(entry.parent!==null){navigate(entry.parent);return "up";}
    if(onRootBack){onRootBack();return "up";}
    return "root";
  },compact);
  function changeSort(key:SpaceSort){if(sort===key)setAscending(v=>!v);else{setSort(key);setAscending(key==="name");}}
  function toggle(id:number){
    if(expanded.has(id)){const next=new Set(expanded);next.delete(id);retained.current=new Set([directory,...next]);setExpanded(next);return;}
    // Cache only a small set of expanded directory pages; all entries remain
    // reachable through pagination and drill-down, regardless of scan size.
    const next=new Set(expanded);
    if(next.size>=8)next.delete(next.values().next().value!);
    next.add(id);setExpanded(next);
    retained.current=new Set([directory,...next]);
    setPages(old=>new Map([...old].filter(([key])=>key===directory||next.has(key))));
    void load(id);
  }
  const sortButton=(key:SpaceSort,label:string)=><button className="disk-explorer__sort" onClick={()=>changeSort(key)}>{label} {sort===key?(ascending?"↑":"↓"):"↕"}</button>;
  function pager(p:SpacePage):ReactNode{
    return <div className="disk-explorer__pager"><span>{p.total.toLocaleString()} 项 · {p.total?`${p.offset+1}–${Math.min(p.offset+p.limit,p.total)}`:"0"}</span>
      <button className="button button--secondary button--small" disabled={p.offset===0||navigating||busy.has(p.directory.id)} onClick={()=>void load(p.directory.id,Math.max(0,p.offset-p.limit))}>上一页</button>
      <button className="button button--secondary button--small" disabled={p.offset+p.limit>=p.total||navigating||busy.has(p.directory.id)} onClick={()=>void load(p.directory.id,p.offset+p.limit)}>下一页</button>
    </div>;
  }
  function rows(p:SpacePage,depth=0):ReactNode{
    return <>{p.entries.map(entry=>{
      const excluded=excludedSpaceEntry(entry);
      const expandable=entry.kind==="directory"&&!excluded;
      const manual=entry.kind==="directory"&&excluded;
      const manualJob=manualJobs.get(entry.id);const manualDetail=manualDetails.get(entry.id);const measured=manualDetail?.result?.nodes[0];
      const unknown=entry.incomplete&&entry.files===0&&entry.directories===0;
      const percent=p.directory[basis]>0?Math.min(100,entry[basis]/p.directory[basis]*100):null;
      const opened=expanded.has(entry.id);const child=pages.get(entry.id);
      const path=directoryPath([...p.ancestors,entry]);
      return <Fragment key={entry.id}><tr>
        <td><div className="disk-explorer__entry disk-explorer__entry--group" style={{color:spaceEntryColor(entry,"label"),paddingLeft:`${Math.min(depth,16)*16}px`}}>
          <button className="disk-explorer__expand" disabled={!expandable&&!manual} aria-label={(manual?(manualOpen.has(entry.id)?"收起目标扫描 ":"展开并扫描 "):opened?"折叠 ":"展开 ")+entry.name} aria-expanded={manual?manualOpen.has(entry.id):expandable?opened:undefined} onClick={()=>manual?toggleManual(entry.id,path):toggle(entry.id)}>{expandable||manual?((manual?manualOpen.has(entry.id):opened)?"▾":"▸"):"·"}</button>
          <button className="disk-explorer__name" title={(manual?"单击展开或收起目标扫描":"单击展开或收起，双击进入目录")+" · "+path} disabled={!expandable&&!manual} aria-expanded={manual?manualOpen.has(entry.id):expandable?expanded.has(entry.id):undefined} onClick={e=>{if(e.detail<2){manual?toggleManual(entry.id,path):toggle(entry.id);}}} onDoubleClick={()=>{if(!manual)navigate(entry.id);}} onKeyDown={e=>{if(!e.repeat&&(e.key==="Enter"||e.key===" ")){e.preventDefault();if(manual)toggleManual(entry.id,path);else if(e.key===" ")toggle(entry.id);else navigate(entry.id);}}}>{entry.kind==="directory"?"▰ ":"▱ "}{entry.name}</button>
          {manual&&<button className="disk-explorer__manual-action" onClick={()=>toggleManual(entry.id,path)}>{manualOpen.has(entry.id)?"收起":"展开扫描"}</button>}
        </div></td><td className="number">{manual&&measured?formatBytes(measured[basis])+(measured.incomplete?" +":""):excluded?(entry.issue==="mount_boundary"?"未扫描":"另处统计"):unknown?"未完整读取":formatBytes(entry[basis])}{entry.incomplete&&!unknown?" +":""}</td>
        <td><div className="disk-explorer__percent"><span>{excluded||unknown||percent===null?"—":`${percent.toFixed(1)}%`}</span><div><i style={{backgroundColor:spaceEntryColor(entry),width:`${excluded||unknown?0:percent??0}%`}}/></div></div></td>
        <td className="number">{manual&&measured?measured.files.toLocaleString():excluded?"—":entry.files.toLocaleString()}{entry.incomplete?" +":""}</td><td className="number">{manual&&measured?measured.directories.toLocaleString():excluded?"—":entry.directories.toLocaleString()}{entry.incomplete?" +":""}</td>
        <td title={manualDetail?"目标单独统计，不计入上级目录":entry.issue?(issues[entry.issue]??entry.issue):entry.incomplete?"下限值":"已读取"} className={entry.incomplete?"disk-explorer__warning":"hint"}>{manual&&manualDetail?(manualDetail.paused?"目标已暂停":manualDetail.status==="running"?"目标扫描中":manualDetail.status==="failed"?"目标扫描失败":"已单独扫描"):entry.issue?(issues[entry.issue]??entry.issue):entry.incomplete?"下限值":"已读取"}{excluded&&<button className="disk-explorer__sort" onClick={()=>onChoosePath(path)}> · 设为单独扫描目录</button>}</td>
      </tr>{manual&&manualOpen.has(entry.id)&&<tr className="disk-explorer__linked-row"><td colSpan={6}><ManualSpaceScan nodeId={nodeId} scanId={manualJob?.id} path={path} starting={manualJob?.starting===true} error={manualJob?.error} onStart={()=>void startManual(entry.id,path)} onDetail={detail=>setManualDetails(old=>new Map(old).set(entry.id,detail))}>
        {detail=><PagedSpaceVisualization key={detail.id} nodeId={nodeId} scanId={detail.id} result={{...detail.result!,paused:detail.paused}} onChoosePath={onChoosePath} hostname={hostname} compact onRootBack={()=>{setManualOpen(old=>{const next=new Set(old);next.delete(entry.id);return next;});back.activate();}}/>}
      </ManualSpaceScan></td></tr>}{opened&&(child?<>{rows(child,depth+1)}{child.total>child.limit&&<tr><td colSpan={6}>{pager(child)}</td></tr>}</>:<tr><td colSpan={6}>正在加载目录…</td></tr>)}</Fragment>;
    })}</>;
  }
  const rectangles=treemapCollapsed?[]:spaceTreemap(page?directorySummaryItems(page,basis):[]);
  return <div className={compact?"disk-explorer__visual disk-explorer__visual--compact":"disk-explorer__visual"} {...back.props}>
    {sensitiveManual&&<SensitiveScanDialog nodeId={nodeId} hostname={hostname} platform={result.platform} request={sensitiveManual.request} onClose={()=>setSensitiveManual(undefined)} onStarted={id=>{const {entry,request}=sensitiveManual;rememberLinkedScan(nodeId,scanId,entry,{id,path:request.path});setManualJobs(old=>new Map(old).set(entry,{id,path:request.path}));setSensitiveManual(undefined);}}/>}
    {!compact&&<>{(result.revision!==undefined||result.status!=="complete")&&<p className="disk-explorer__notice">{result.paused?"扫描已暂停，已有目录结果可继续查看。":result.status==="complete"?"扫描完成，显示本次扫描的目录汇总。":<>{reasons[result.reason]??"扫描未完成"}。大小为已读取部分的下限，百分比仅相对于已扫描数据。</>}</p>}
    <div className="disk-explorer__summary"><div><span>实际占用</span><strong>{formatBytes(current.allocated)}{current.incomplete?" +":""}</strong></div><div><span>文件大小</span><strong>{formatBytes(current.apparent)}{current.incomplete?" +":""}</strong></div><div><span>文件 / 子目录</span><strong>{current.files.toLocaleString()} / {current.directories.toLocaleString()}</strong></div><div><span>扫描身份</span><strong>{result.username} · {result.native_id?`SID ${result.native_id}`:`UID ${result.uid}`}</strong></div></div>
    {result.detail_level==="directories"&&<p className="hint">仅汇总目录，文件大小和数量已计入各级目录；不保留文件名和逐文件明细。{result.saved_directories?.toLocaleString()} 个目录记录。</p>}
    {page?.direct&&<p className="disk-explorer__direct">本层文件及目录自身：<strong>{formatBytes(page.direct[basis])}{current.incomplete?" +":""}</strong> · {page.direct.files.toLocaleString()} 个直接文件 · 占当前目录 {current[basis]>0?(100*page.direct[basis]/current[basis]).toFixed(1):"0.0"}%<span className="hint">（包含目录元数据及未跟随的符号链接，不含子目录）</span></p>}
    </>}
    {result.scope==="all_local"&&<div className="disk-explorer__shortcuts"><span>本地文件系统</span><button className="button button--secondary button--small" onClick={()=>navigate(0)}>总览</button>{result.filesystems?.map(fs=><button key={fs.id} className="button button--secondary button--small" onClick={()=>navigate(fs.id)}>{fs.path} · {fs.filesystem}{fs.incomplete?" · 不完整":""}</button>)}</div>}
    <nav className="disk-explorer__breadcrumbs" aria-label="目录层级">{(page?.ancestors??[current]).map((entry,index)=><span key={entry.id}>{index>0&&" / "}<button disabled={entry.id===directory} onClick={()=>navigate(entry.id)}>{entry.id===0&&result.scope==="all_local"?"全部本地磁盘":entry.name}</button></span>)}</nav>
    <div className="disk-explorer__toolbar"><label>统计口径<select value={basis} onChange={e=>setBasis(e.target.value as SpaceBasis)}><option value="allocated">实际磁盘占用</option><option value="apparent">文件大小（逻辑大小）</option></select></label>
      <button className="button button--secondary button--small" onClick={()=>{retained.current=new Set([directory]);setExpanded(new Set());setPages(old=>new Map([...old].filter(([id])=>id===directory)));}}>折叠全部</button>
      <button className="button button--secondary button--small" disabled={!page||navigating||(directory===0&&result.scope==="all_local")} onClick={()=>page&&onChoosePath(directoryPath(page.ancestors))}>将此目录设为扫描范围</button>
      <span className="hint">每页 250 个子目录。{result.revision===undefined?"排序作用于全部子目录；可逐页查看。":"实时刷新保持行位置，点击表头重新排序。"}</span>
      {!compact&&<button type="button" className="button button--secondary button--small disk-explorer__treemap-toggle" aria-expanded={!treemapCollapsed} aria-controls={clipPrefix+"-treemap"} onClick={()=>setTreemapCollapsed(v=>!v)}>{treemapCollapsed?"展开本图":"折叠本图"}</button>}
    </div>
    {!compact&&<div id={clipPrefix+"-treemap"} hidden={treemapCollapsed}>{page&&<svg className="disk-explorer__treemap" viewBox="0 0 1000 300" aria-label="当前目录空间占用矩形图">
      {rectangles.map((rect,index)=>{const entry=page.top.find(e=>e.id===rect.id);const actionable=entry?.kind==="directory"&&!excludedSpaceEntry(entry);
        return <g key={rect.id} role={actionable?"button":undefined} tabIndex={actionable?0:undefined} aria-label={`${rect.name} ${formatBytes(rect.value)}`} onClick={()=>actionable&&navigate(rect.id)} onKeyDown={e=>{if(actionable&&(e.key==="Enter"||e.key===" ")){e.preventDefault();navigate(rect.id);}}}>
          <title>{rect.name} · {formatBytes(rect.value)} · {(rect.value/current[basis]*100).toFixed(1)}%</title>
          <defs><clipPath id={`${clipPrefix}-${index}`}><rect x={rect.x+5} y={rect.y+3} width={Math.max(0,rect.width-10)} height={Math.max(0,rect.height-6)}/></clipPath></defs>
          <rect x={rect.x} y={rect.y} width={rect.width} height={rect.height} fill={`var(--space-color-${((rect.id%6)+6)%6})`} stroke="var(--surface)" strokeWidth={2}/>
          {rect.width>60&&rect.height>28&&<g clipPath={`url(#${clipPrefix}-${index})`}><text x={rect.x+9} y={rect.y+20}>{rect.name}</text>{rect.height>48&&<text x={rect.x+9} y={rect.y+40} className="disk-explorer__treemap-size">{formatBytes(rect.value)}</text>}</g>}
        </g>;})}
    </svg>}</div>}
    <div className="disk-explorer__filter"><label>筛选子目录<input value={search} maxLength={128} onChange={e=>setSearch(e.target.value)} placeholder="搜索子目录名称"/></label><span className="hint">矩形图显示前 40 个子目录，其余合并；本层文件占用单独汇总。</span></div>
    {error&&<p className="form-error" role="alert">{error} <button className="button button--secondary button--small" onClick={()=>void load(directory)}>重试加载</button></p>}{busy.has(directory)&&!page&&<p role="status">正在加载目录…</p>}
    {page&&<><div className="table-scroll disk-explorer__page-table" aria-busy={busy.has(directory)}>{busy.has(directory)&&<span className="disk-explorer__updating" role="status">正在更新目录…</span>}<table className="disk-explorer__tree disk-explorer__tree--paged"><colgroup><col style={{width:"34%"}}/><col style={{width:"15%"}}/><col style={{width:"15%"}}/><col style={{width:"11%"}}/><col style={{width:"11%"}}/><col style={{width:"14%"}}/></colgroup><thead><tr><th><div className="disk-explorer__name-heading">{sortButton("name","名称 / 层级")}<ParentDirectoryButton parent={current.parent} onNavigate={navigate} disabled={navigating} onRootBack={onRootBack}/></div></th><th>{sortButton("size",basis==="allocated"?"实际占用":"文件大小")}</th><th>{sortButton("percent","占上一层")}</th><th>{sortButton("files","文件数")}</th><th>子目录</th><th>状态</th></tr></thead><tbody>{rows(page)}</tbody></table></div>{!page.entries.length&&<p className="table-empty">{filter?"没有匹配的子目录。":"该目录没有子目录；本层文件已计入上方汇总。"}</p>}{pager(page)}</>}
    <p className="hint">{result.visited.toLocaleString()} 个已扫描条目 · {result.errors} 个读取问题 · {result.skipped_mounts} 个挂载边界 · {result.symlinks} 个符号链接 · {result.hardlinks} 个重复硬链接。{result.scope==="all_local"?"各本地文件系统分别扫描，绑定挂载去重；总览包含本轮实际读取的数据。":"指定目录不跨挂载点。"}硬链接只计一次；稀疏文件、共享块、压缩与快照可能使结果与磁盘总使用量不同。</p>
  </div>;
}

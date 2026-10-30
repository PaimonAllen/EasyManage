import {Fragment,useCallback,useEffect,useId,useRef,useState,type ReactNode} from "react";
import {fetchSpacePage,type SpacePage,type SpaceResult} from "../api/diskSpace";
import {spaceTreemap,spaceDirectoryPath,excludedSpaceEntry,directorySummaryItems,type SpaceBasis,type SpaceSort} from "../lib/diskSpace";
import {formatBytes} from "../lib/format";

const issues:Record<string,string>={permission_denied:"无权读取",changed_or_denied:"条目变化或无法读取",unrepresentable_name:"文件名编码不支持",path_limit:"路径长度达到限制",depth_limit:"达到层级限制",hardlink:"硬链接，已在别处计数",symlink:"符号链接，不跟随",mount_boundary:"挂载点，未扫描",duplicate_mount:"绑定挂载，在其他位置统计",separate_filesystem:"独立文件系统，见磁盘总览",mount_changed:"挂载发生变化，请重新扫描",no_local_filesystems:"没有可扫描的本地文件系统",special:"特殊文件"};
const reasons:Record<string,string>={time_limit:"达到所选时限",entry_limit:"达到所选条目上限",cancelled:"手动取消",unreadable_entries:"部分目录读取受限",platform_storage_low:"平台结果存储空间不足，已停止扫描以保护数据库",platform_storage_unknown:"无法确认平台存储可用空间，已安全停止扫描",memory_limit:"扫描内存达到保护上限"};
const directoryPath=spaceDirectoryPath;

export function PagedSpaceVisualization({nodeId,scanId,result,onChoosePath}:{nodeId:string;scanId:string;result:SpaceResult;onChoosePath:(path:string)=>void}){
  const [directory,setDirectory]=useState(0);const [basis,setBasis]=useState<SpaceBasis>("allocated");
  const [sort,setSort]=useState<SpaceSort>("size");const [ascending,setAscending]=useState(false);
  const [search,setSearch]=useState("");const [filter,setFilter]=useState("");
  const [pages,setPages]=useState<Map<number,SpacePage>>(new Map());const [expanded,setExpanded]=useState<Set<number>>(new Set());
  const [busy,setBusy]=useState<Set<number>>(new Set());const [error,setError]=useState("");
  const control=useRef<AbortController|null>(null);const sequence=useRef(new Map<number,number>());
  const retained=useRef(new Set<number>());
  const clipPrefix=useId().replace(/:/g,"");
  useEffect(()=>{const timer=setTimeout(()=>setFilter(search),250);return()=>clearTimeout(timer);},[search]);
  const load=useCallback(async(parent:number,offset=0)=>{
    const controller=control.current;if(!controller)return;
    const serial=(sequence.current.get(parent)??0)+1;sequence.current.set(parent,serial);
    setBusy(old=>new Set(old).add(parent));setError("");
    try{
      const page=await fetchSpacePage(nodeId,scanId,parent,basis,sort,ascending,offset,parent===directory?filter:"",controller.signal);
      if(!controller.signal.aborted&&sequence.current.get(parent)===serial&&retained.current.has(parent))setPages(old=>new Map(old).set(parent,page));
    }catch(e){if(!controller.signal.aborted)setError(e instanceof Error?e.message:"目录加载失败");}
    finally{if(!controller.signal.aborted&&sequence.current.get(parent)===serial)setBusy(old=>{const next=new Set(old);next.delete(parent);return next;});}
  },[nodeId,scanId,basis,sort,ascending,directory,filter]);
  useEffect(()=>{
    const controller=new AbortController();control.current=controller;sequence.current=new Map();
    retained.current=new Set([directory]);
    setPages(new Map());setExpanded(new Set());setBusy(new Set());void load(directory);
    return()=>controller.abort();
  },[load,directory]);
  const page=pages.get(directory);const current=page?.directory??result.nodes[0];
  function navigate(id:number){setDirectory(id);setSearch("");setFilter("");}
  function changeSort(key:SpaceSort){if(sort===key)setAscending(v=>!v);else{setSort(key);setAscending(key==="name");}}
  function toggle(id:number){
    if(expanded.has(id)){setExpanded(old=>{const next=new Set(old);next.delete(id);return next;});return;}
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
      <button className="button button--secondary button--small" disabled={p.offset===0||busy.has(p.directory.id)} onClick={()=>void load(p.directory.id,Math.max(0,p.offset-p.limit))}>上一页</button>
      <button className="button button--secondary button--small" disabled={p.offset+p.limit>=p.total||busy.has(p.directory.id)} onClick={()=>void load(p.directory.id,p.offset+p.limit)}>下一页</button>
    </div>;
  }
  function rows(p:SpacePage,depth=0):ReactNode{
    return <>{p.entries.map(entry=>{
      const excluded=excludedSpaceEntry(entry);
      const expandable=entry.kind==="directory"&&!excluded;
      const unknown=entry.incomplete&&entry.files===0&&entry.directories===0;
      const percent=p.directory[basis]>0?Math.min(100,entry[basis]/p.directory[basis]*100):null;
      const opened=expanded.has(entry.id);const child=pages.get(entry.id);
      const path=directoryPath([...p.ancestors,entry]);
      return <Fragment key={entry.id}><tr>
        <td><div className="disk-explorer__entry" style={{paddingLeft:`${Math.min(depth,16)*16}px`}}>
          <button className="disk-explorer__expand" disabled={!expandable} aria-label={`${opened?"折叠":"展开"} ${entry.name}`} aria-expanded={expandable?opened:undefined} onClick={()=>toggle(entry.id)}>{expandable?(opened?"▾":"▸"):"·"}</button>
          <button className="disk-explorer__name" title={path} disabled={!expandable} onClick={()=>navigate(entry.id)}>{entry.kind==="directory"?"▰ ":"▱ "}{entry.name}</button>
        </div></td><td className="number">{excluded?(entry.issue==="mount_boundary"?"未扫描":"另处统计"):unknown?"未完整读取":formatBytes(entry[basis])}{entry.incomplete&&!unknown?" +":""}</td>
        <td><div className="disk-explorer__percent"><span>{excluded||unknown||percent===null?"—":`${percent.toFixed(1)}%`}</span><div><i style={{width:`${excluded||unknown?0:percent??0}%`}}/></div></div></td>
        <td className="number">{excluded?"—":entry.files.toLocaleString()}{entry.incomplete?" +":""}</td><td className="number">{excluded?"—":entry.directories.toLocaleString()}{entry.incomplete?" +":""}</td>
        <td className={entry.incomplete?"disk-explorer__warning":"hint"}>{entry.issue?(issues[entry.issue]??entry.issue):entry.incomplete?"下限值":"已读取"}{excluded&&<button className="disk-explorer__sort" onClick={()=>onChoosePath(path)}> · 设为单独扫描目录</button>}</td>
      </tr>{opened&&(child?<>{rows(child,depth+1)}{child.total>child.limit&&<tr><td colSpan={6}>{pager(child)}</td></tr>}</>:<tr><td colSpan={6}>正在加载目录…</td></tr>)}</Fragment>;
    })}</>;
  }
  const rectangles=spaceTreemap(page?directorySummaryItems(page,basis):[]);
  return <div className="disk-explorer__visual">
    {result.status!=="complete"&&<p className="disk-explorer__notice">{reasons[result.reason]??"扫描未完成"}。大小为已读取部分的下限，百分比仅相对于已扫描数据。</p>}
    <div className="disk-explorer__summary"><div><span>实际占用</span><strong>{formatBytes(current.allocated)}{current.incomplete?" +":""}</strong></div><div><span>文件大小</span><strong>{formatBytes(current.apparent)}{current.incomplete?" +":""}</strong></div><div><span>文件 / 子目录</span><strong>{current.files.toLocaleString()} / {current.directories.toLocaleString()}</strong></div><div><span>扫描身份</span><strong>{result.username} · UID {result.uid}</strong></div></div>
    {result.detail_level==="directories"&&<p className="hint">仅保存目录汇总，文件大小和数量已计入各级目录；不保存文件名和逐文件明细。{result.saved_directories?.toLocaleString()} 个目录记录。</p>}
    {page?.direct&&<p className="disk-explorer__direct">本层文件及目录自身：<strong>{formatBytes(page.direct[basis])}{current.incomplete?" +":""}</strong> · {page.direct.files.toLocaleString()} 个直接文件 · 占当前目录 {current[basis]>0?(100*page.direct[basis]/current[basis]).toFixed(1):"0.0"}%<span className="hint">（包含目录元数据及未跟随的符号链接，不含子目录）</span></p>}
    {result.scope==="all_local"&&<div className="disk-explorer__shortcuts"><span>本地文件系统</span><button className="button button--secondary button--small" onClick={()=>navigate(0)}>总览</button>{result.filesystems?.map(fs=><button key={fs.id} className="button button--secondary button--small" onClick={()=>navigate(fs.id)}>{fs.path} · {fs.filesystem}{fs.incomplete?" · 不完整":""}</button>)}</div>}
    <nav className="disk-explorer__breadcrumbs" aria-label="目录层级">{(page?.ancestors??[current]).map((entry,index)=><span key={entry.id}>{index>0&&" / "}<button disabled={entry.id===directory} onClick={()=>navigate(entry.id)}>{entry.id===0&&result.scope==="all_local"?"全部本地磁盘":entry.name}</button></span>)}</nav>
    <div className="disk-explorer__toolbar"><label>统计口径<select value={basis} onChange={e=>setBasis(e.target.value as SpaceBasis)}><option value="allocated">实际磁盘占用</option><option value="apparent">文件大小（逻辑大小）</option></select></label>
      <button className="button button--secondary button--small" onClick={()=>{retained.current=new Set([directory]);setExpanded(new Set());setPages(old=>new Map([...old].filter(([id])=>id===directory)));}}>折叠全部</button>
      <button className="button button--secondary button--small" disabled={!page||(directory===0&&result.scope==="all_local")} onClick={()=>page&&onChoosePath(directoryPath(page.ancestors))}>将此目录设为扫描范围</button>
      <span className="hint">每页 250 个子目录，排序作用于全部子目录；可逐页查看。</span>
    </div>
    {page&&rectangles.length>0&&<svg className="disk-explorer__treemap" viewBox="0 0 1000 300" aria-label="当前目录空间占用矩形图">
      {rectangles.map((rect,index)=>{const entry=page.top.find(e=>e.id===rect.id);const actionable=entry?.kind==="directory"&&!excludedSpaceEntry(entry);
        return <g key={rect.id} role={actionable?"button":undefined} tabIndex={actionable?0:undefined} aria-label={`${rect.name} ${formatBytes(rect.value)}`} onClick={()=>actionable&&navigate(rect.id)} onKeyDown={e=>{if(actionable&&(e.key==="Enter"||e.key===" ")){e.preventDefault();navigate(rect.id);}}}>
          <title>{rect.name} · {formatBytes(rect.value)} · {(rect.value/current[basis]*100).toFixed(1)}%</title>
          <defs><clipPath id={`${clipPrefix}-${index}`}><rect x={rect.x+5} y={rect.y+3} width={Math.max(0,rect.width-10)} height={Math.max(0,rect.height-6)}/></clipPath></defs>
          <rect x={rect.x} y={rect.y} width={rect.width} height={rect.height} fill={`var(--space-color-${index%6})`} stroke="var(--surface)" strokeWidth={2}/>
          {rect.width>60&&rect.height>28&&<g clipPath={`url(#${clipPrefix}-${index})`}><text x={rect.x+9} y={rect.y+20}>{rect.name}</text>{rect.height>48&&<text x={rect.x+9} y={rect.y+40} className="disk-explorer__treemap-size">{formatBytes(rect.value)}</text>}</g>}
        </g>;})}
    </svg>}
    <div className="disk-explorer__filter"><label>筛选子目录<input value={search} maxLength={128} onChange={e=>setSearch(e.target.value)} placeholder="搜索子目录名称"/></label><span className="hint">矩形图显示前 40 个子目录，其余合并；本层文件占用单独汇总。</span></div>
    {error&&<p className="form-error" role="alert">{error}</p>}{busy.has(directory)&&<p role="status">正在加载目录…</p>}
    {page&&<><div className="table-scroll"><table className="disk-explorer__tree"><thead><tr><th>{sortButton("name","名称 / 层级")}</th><th>{sortButton("size",basis==="allocated"?"实际占用":"文件大小")}</th><th>{sortButton("percent","占上一层")}</th><th>{sortButton("files","文件数")}</th><th>子目录</th><th>状态</th></tr></thead><tbody>{rows(page)}</tbody></table></div>{!page.entries.length&&<p className="table-empty">{filter?"没有匹配的子目录。":"该目录没有子目录；本层文件已计入上方汇总。"}</p>}{pager(page)}</>}
    <p className="hint">{result.visited.toLocaleString()} 个已扫描条目 · {result.errors} 个读取问题 · {result.skipped_mounts} 个挂载边界 · {result.symlinks} 个符号链接 · {result.hardlinks} 个重复硬链接。{result.scope==="all_local"?"各本地文件系统分别扫描，绑定挂载去重；总览包含本轮实际读取的数据。":"指定目录不跨挂载点。"}硬链接只计一次；稀疏文件、共享块、压缩与快照可能使结果与磁盘总使用量不同。</p>
  </div>;
}

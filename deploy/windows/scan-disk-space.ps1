# Read-only, metadata-only directory scanner. Never opens file contents.
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$source=@"
using System;
using System.IO;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Diagnostics;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;
using System.Security.Principal;
public static class EasyManageWindowsScan {
 [StructLayout(LayoutKind.Sequential)] struct Standard { public long allocation,end;public uint links;public byte delete,directory; }
 [StructLayout(LayoutKind.Sequential)] struct Attributes {public uint flags,tag;}
 [StructLayout(LayoutKind.Sequential)] struct Identity { public ulong volume;public Guid id; }
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]
 static extern SafeFileHandle CreateFileW(string name,uint access,uint share,IntPtr security,uint creation,uint flags,IntPtr template);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetFileInformationByHandleEx(SafeFileHandle handle,int cls,out Standard value,uint size);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetFileInformationByHandleEx(SafeFileHandle handle,int cls,out Identity value,uint size);
 [DllImport("kernel32.dll",SetLastError=true)] static extern bool GetFileInformationByHandleEx(SafeFileHandle handle,int cls,out Attributes value,uint size);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern uint GetFinalPathNameByHandleW(SafeFileHandle handle,System.Text.StringBuilder path,uint size,uint flags);
 public sealed class Node {
  public long id {get;set;} public long? parent {get;set;} public string name {get;set;} public string kind {get;set;}="directory";
  public long allocated {get;set;} public long apparent {get;set;} public long files {get;set;} public long directories {get;set;}
  public bool incomplete {get;set;} public string issue {get;set;} public long mtime {get;set;} public string scan_path {get;set;}
 }
 sealed class Scan {
  public string root,scope,user,principal;public bool admin;public long max,visited,errors,next,symlinks,hardlinks,skipped_mounts=0;public int seconds;
  public Stopwatch watch=Stopwatch.StartNew();public string reason="";
  public HashSet<string> links=new HashSet<string>();public volatile bool cancelled, pauseRequested;public bool paused;public long lastProgress;public double completedWork;public bool sensitiveAuthorized;
  void Emit(object value){Console.WriteLine(JsonSerializer.Serialize(value));Console.Out.Flush();}
  public bool Check(){
   while(pauseRequested&&!cancelled){
    if(!paused){watch.Stop();paused=true;Emit(new{type="control",paused=true});}
    System.Threading.Thread.Sleep(100);
   }
   if(paused&&!cancelled){watch.Start();paused=false;Emit(new{type="control",paused=false});}
   if(cancelled){reason="cancelled";return false;}
   if(seconds>0&&watch.Elapsed.TotalSeconds>=seconds){reason="time_limit";return false;}
   if(max>0&&visited>=max){reason="entry_limit";return false;}return true;
  }
  void Publish(Node node){Emit(new{type="entries",nodes=new[]{node}});}
  void Progress(){if(watch.ElapsedMilliseconds-lastProgress>1000){lastProgress=watch.ElapsedMilliseconds;Emit(new{type="progress",visited,errors,estimated_percent=Math.Min(99.9,completedWork*100),elapsed_seconds=watch.Elapsed.TotalSeconds});System.Threading.Thread.Sleep(1);}}
  bool Metadata(string path,string boundary,out Standard data,out string identity){
   data=default;identity=null;
   using(var handle=CreateFileW(path,128,7,IntPtr.Zero,3,0x02200000,IntPtr.Zero)){
    if(handle.IsInvalid)return false;
    Attributes attrs;if(!GetFileInformationByHandleEx(handle,9,out attrs,(uint)Marshal.SizeOf<Attributes>()) || (attrs.flags&0x400)!=0)return false;
    var final=new System.Text.StringBuilder(32768);
    if(GetFinalPathNameByHandleW(handle,final,32768,0)==0)return false;
    string canonical=final.ToString();if(canonical.StartsWith(@"\\?\"))canonical=canonical.Substring(4);
    string actual=canonical.Replace('\\','/').TrimEnd('/');
    string expected=Path.GetFullPath(path).Replace('\\','/').TrimEnd('/');
    if(!actual.Equals(expected,StringComparison.OrdinalIgnoreCase))return false;
    if(!actual.Equals(boundary,StringComparison.OrdinalIgnoreCase)&&!actual.StartsWith(boundary+"/",StringComparison.OrdinalIgnoreCase))return false;
    if(!GetFileInformationByHandleEx(handle,1,out data,(uint)Marshal.SizeOf<Standard>()))return false;
    if(data.links>1){Identity id;if(!GetFileInformationByHandleEx(handle,18,out id,(uint)Marshal.SizeOf<Identity>()))return false;identity=id.volume+":"+id.id;}
    return true;
   }
  }
  public Node Walk(string path,string name,long? parent,int depth,string boundary,string scanPath=null,double start=0,double weight=1){
   var node=new Node{id=next++,parent=parent,name=name,scan_path=scanPath};visited++;Publish(node);
   try{
    var dir=new DirectoryInfo(path);
    if((dir.Attributes&FileAttributes.ReparsePoint)!=0){node.issue="reparse_point";node.incomplete=true;return node;}
    Standard meta;string ident;if(!Metadata(path,boundary,out meta,out ident)){node.issue="unreadable";node.incomplete=true;errors++;return node;}
    node.allocated=Math.Max(0,meta.allocation);node.apparent=Math.Max(0,meta.end);node.mtime=new DateTimeOffset(dir.LastWriteTimeUtc).ToUnixTimeSeconds();
    if(depth>=64){node.issue="depth_limit";node.incomplete=true;return node;}
    long count=0,index=0;
    foreach(var ignored in dir.EnumerateFileSystemInfos()){if(!Check()){node.incomplete=true;return node;}count++;if(count%256==0)Progress();}
    double step=weight/(count+1);
    foreach(var entry in dir.EnumerateFileSystemInfos()){
     if(!Check()){node.incomplete=true;break;}Progress();
     double entryStart=start+index++*step;
     try{
      if((entry.Attributes&FileAttributes.ReparsePoint)!=0){visited++;symlinks++;node.incomplete=true;continue;}
      if((entry.Attributes&FileAttributes.Directory)!=0){
       Standard directory;string directoryKey;if(!Metadata(entry.FullName,boundary,out directory,out directoryKey)){visited++;errors++;node.incomplete=true;continue;}
       var child=Walk(entry.FullName,entry.Name,node.id,depth+1,boundary,null,entryStart,step);
       node.allocated+=child.allocated;node.apparent+=child.apparent;node.files+=child.files;node.directories+=child.directories+1;node.incomplete|=child.incomplete;
      }else{
       visited++;Standard file;string key;if(!Metadata(entry.FullName,boundary,out file,out key)){errors++;node.incomplete=true;continue;}
       node.files++;if(key!=null&&links.Count>=1000000){reason="memory_limit";node.incomplete=true;break;}
       if(key==null||links.Add(key)){node.allocated+=Math.Max(0,file.allocation);node.apparent+=Math.Max(0,file.end);}else{hardlinks++;}
      }
     }catch{errors++;node.incomplete=true;}
     finally{if(reason=="")completedWork=Math.Max(completedWork,Math.Min(start+weight,entryStart+step));}
    }
   }catch{errors++;node.incomplete=true;node.issue="unreadable";}
   finally{if(reason=="")completedWork=Math.Max(completedWork,start+weight);Publish(node);}
   return node;
  }
  static bool Sensitive(string path,string scanScope){
   if(scanScope=="all_local")return true;
   var parts=path.Replace('\\','/').Substring(3).Split(new[]{'/'},StringSplitOptions.RemoveEmptyEntries);
   if(parts.Length<=1)return true;
   string first=parts[0].ToLowerInvariant();
   return new HashSet<string>{"windows","program files","program files (x86)","programdata","system volume information","recovery","$recycle.bin"}.Contains(first);
  }
  public void Run(){
   if(!sensitiveAuthorized&&Sensitive(root,scope))throw new Exception("Administrator review required");
   var id=WindowsIdentity.GetCurrent();user=id.Name;principal=id.User.Value;admin=new WindowsPrincipal(id).IsInRole(WindowsBuiltInRole.Administrator);
   Task.Run(()=>{while(true){int c=Console.In.Read();if(c==112)pauseRequested=true;else if(c==114)pauseRequested=false;else{cancelled=true;break;}}});
   Emit(new{type="metadata",root,scope,uid=uint.MaxValue,username=user,platform="windows",native_id=principal,administrator=admin});
   Node top;var filesystems=new List<object>();
   if(scope=="all_local"){
    top=new Node{id=next++,name=root};visited++;Publish(top);
    var targets=new List<DriveInfo>();foreach(var item in DriveInfo.GetDrives())if(item.DriveType==DriveType.Fixed&&item.IsReady)targets.Add(item);
    int targetIndex=0;foreach(var drive in targets){
     if(!Check()){top.incomplete=true;break;}
     if(drive.DriveType!=DriveType.Fixed||!drive.IsReady)continue;
     string path=drive.RootDirectory.FullName;string name=path.Replace('\\','/');
     var child=Walk(path,name,top.id,0,name.TrimEnd('/'),name,(double)targetIndex++/targets.Count,1.0/targets.Count);
     top.allocated+=child.allocated;top.apparent+=child.apparent;top.files+=child.files;top.directories+=child.directories+1;top.incomplete|=child.incomplete;
     filesystems.Add(new{id=child.id,path=name,filesystem=drive.DriveFormat,incomplete=child.incomplete});
    }
    Publish(top);
   }else{
    string path=Path.GetFullPath(root);var drive=new DriveInfo(Path.GetPathRoot(path));
    if(drive.DriveType!=DriveType.Fixed||!drive.IsReady)throw new Exception("Local fixed drive required");
    var part=new DirectoryInfo(path);
    while(part!=null){if((part.Attributes&FileAttributes.ReparsePoint)!=0)throw new Exception("Reparse point refused");part=part.Parent;}
    top=Walk(path,root,null,0,path.Replace('\\','/').TrimEnd('/'));
   }
   Emit(new{type="result",root,scope,uid=uint.MaxValue,username=user,platform="windows",native_id=principal,administrator=admin,
    status=reason=="cancelled"?"cancelled":reason!=""||top.incomplete?"partial":"complete",reason=reason!=""?reason:top.incomplete?"unreadable_entries":"",
    estimated_percent=reason==""&&!top.incomplete?100:Math.Min(99.9,completedWork*100),elapsed_seconds=watch.Elapsed.TotalSeconds,filesystem=scope=="all_local"?"multiple":new DriveInfo(Path.GetPathRoot(root)).DriveFormat,
    filesystems,visited,errors,symlinks,hardlinks,skipped_mounts,detail_level="directories",saved_directories=next,paged=true,nodes=new[]{top}});
  }
 }
 public static void Run(string root,string scope,string mode,int seconds,long max,bool approvedSensitive){
  bool admin=new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator);
  if(admin!=(mode=="full"))throw new Exception("Actual Windows privilege differs from scan mode");
  try{Process.GetCurrentProcess().PriorityClass=ProcessPriorityClass.Idle;}catch{}
  new Scan{root=root,scope=scope,seconds=seconds,max=max,sensitiveAuthorized=approvedSensitive}.Run();
 }
}
"@
try {
 Add-Type -TypeDefinition $source
 [EasyManageWindowsScan]::Run($request.path,$request.scope,$request.mode,[int]$request.seconds,[long]$request.max_entries,[bool]$request.approved_sensitive)
} catch {[Console]::Error.WriteLine('Windows metadata scan failed or was refused; no source files changed');exit 1}

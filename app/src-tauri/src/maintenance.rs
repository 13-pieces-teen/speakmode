//! Local installation maintenance. A second invocation never creates a WebView,
//! begins capture or replays a result. Deletion is explicit and follows a verified
//! resource-aware exit of the exact same executable in the same Windows account.
use std::{future::Future, io, path::{Path,PathBuf}, pin::Pin, sync::{Arc,atomic::{AtomicBool,Ordering}}};
use tokio_util::sync::CancellationToken;

pub const PROFILE_MARKER:&str=".speakmode-profile.json";
pub const APP_IDENTIFIER:&str="dev.speakmode.desktop";
const REQUEST:&[u8]=b"SpeakMode/maintenance/1/prepare\n";
const READY:&[u8]=b"READY\n";
const FAILED:&[u8]=b"FAIL!\n";
const ACK:&[u8]=b"ACK\n";
pub type PrepareFuture=Pin<Box<dyn Future<Output=io::Result<()>>+Send>>;
pub type Prepare=Arc<dyn Fn()->PrepareFuture+Send+Sync>;

#[derive(Debug,Default,PartialEq,Eq)]
pub struct Options {pub delete_data:bool,pub delete_key:bool}
pub fn parse_arguments(args:impl IntoIterator<Item=String>)->io::Result<Option<Options>> {
    let args:Vec<_>=args.into_iter().collect();
    let maintenance=args.iter().any(|arg|matches!(arg.as_str(),"--prepare-uninstall"|"--delete-user-data"|"--delete-cloud-key"));
    if !maintenance{return Ok(None);}
    if !args.iter().any(|arg|arg=="--prepare-uninstall")||args.iter().any(|arg|!matches!(arg.as_str(),"--prepare-uninstall"|"--delete-user-data"|"--delete-cloud-key")) {
        return Err(io::Error::other("invalid maintenance arguments"));
    }
    Ok(Some(Options{delete_data:args.iter().any(|a|a=="--delete-user-data"),delete_key:args.iter().any(|a|a=="--delete-cloud-key")}))
}

pub struct Server {cancel:CancellationToken,released:Arc<AtomicBool>}
impl Server {
    pub fn stop(&self){self.cancel.cancel();}
    pub fn released(&self)->bool{self.released.load(Ordering::Acquire)}
}
impl Drop for Server {fn drop(&mut self){self.cancel.cancel();}}

/// Startup writes only an ownership marker, never history or a credential.
pub fn mark_profile(root:&Path)->io::Result<()> {
    let marker=root.join(PROFILE_MARKER);
    if marker.exists(){verify_marker(root)?;return Ok(());}
    use std::io::Write;
    let mut file=std::fs::OpenOptions::new().create_new(true).write(true).open(marker)?;
    file.write_all(br#"{"schema":1,"identifier":"dev.speakmode.desktop"}"#)?;
    file.sync_all()
}
fn verify_marker(root:&Path)->io::Result<()> {
    let marker=root.join(PROFILE_MARKER);
    let meta=std::fs::symlink_metadata(&marker)?;
    if !meta.is_file()||is_reparse(&meta)||meta.len()>4096{return Err(io::Error::other("invalid profile marker"));}
    let value:serde_json::Value=serde_json::from_slice(&std::fs::read(marker)?).map_err(io::Error::other)?;
    if value["schema"]!=1||value["identifier"]!=APP_IDENTIFIER{return Err(io::Error::other("profile is not owned by SpeakMode"));}
    Ok(())
}
fn is_reparse(meta:&std::fs::Metadata)->bool {
    #[cfg(windows)] {use std::os::windows::fs::MetadataExt;meta.file_attributes()&0x400!=0}
    #[cfg(not(windows))] {meta.file_type().is_symlink()}
}
/// Reject every reparse point, including ancestors, before recursive cleanup.
/// The caller provides the known application root; no user-supplied delete path.
pub fn validate_data_root(root:&Path,expected:&Path)->io::Result<Option<PathBuf>> {
    if !root.is_absolute()||!expected.is_absolute()||root!=expected{return Err(io::Error::other("unexpected data directory"));}
    if !root.exists(){return Ok(None);}
    for ancestor in root.ancestors(){if is_reparse(&std::fs::symlink_metadata(ancestor)?){return Err(io::Error::other("reparse data path refused"));}}
    let resolved=root.canonicalize()?;
    if resolved.parent().is_none()||root.file_name().is_none(){return Err(io::Error::other("root directory refused"));}
    verify_marker(&resolved)?;
    let mut pending=vec![resolved.clone()];
    while let Some(path)=pending.pop(){
        for entry in std::fs::read_dir(path)? {
            let entry=entry?;let meta=std::fs::symlink_metadata(entry.path())?;
            if is_reparse(&meta){return Err(io::Error::other("reparse point inside profile refused"));}
            if meta.is_dir(){pending.push(entry.path());}
        }
    }
    Ok(Some(resolved))
}
pub fn delete_data(root:&Path,expected:&Path)->io::Result<()> {
    if let Some(path)=validate_data_root(root,expected)? {
        // Keep ownership evidence through partial failures (for example a
        // WebView file still held by another process), so cleanup is retryable.
        for entry in std::fs::read_dir(&path)? {
            let entry=entry?;if entry.file_name()==PROFILE_MARKER{continue;}
            let meta=std::fs::symlink_metadata(entry.path())?;
            if is_reparse(&meta){return Err(io::Error::other("reparse point inside profile refused"));}
            if meta.is_dir(){std::fs::remove_dir_all(entry.path())?;}else{std::fs::remove_file(entry.path())?;}
        }
        std::fs::remove_file(path.join(PROFILE_MARKER))?;
        if let Err(error)=std::fs::remove_dir(&path){
            if !is_reparse(&std::fs::symlink_metadata(&path)?)&&path.canonicalize()?==path{mark_profile(&path)?;}
            return Err(error);
        }
    }
    Ok(())
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum VerdictDelivery {Acknowledged,Unconfirmed}

#[cfg(windows)]
pub use native::{start_server,prepare_uninstall};
#[cfg(not(any(windows, target_os="macos")))]
pub async fn prepare_uninstall()->io::Result<()>{Err(io::Error::other("Windows maintenance only"))}

#[cfg(windows)]
mod native {
    use super::*;
    use sha2::{Digest,Sha256};
    use std::{os::windows::io::AsRawHandle,time::Duration};
    use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::windows::named_pipe::{ServerOptions,ClientOptions}};
    use windows::{core::{PCWSTR,PWSTR},Win32::{Foundation::{CloseHandle,HANDLE,HLOCAL,LocalFree,WAIT_OBJECT_0},
        Security::{GetTokenInformation,TOKEN_QUERY,TokenUser,TOKEN_USER,SECURITY_ATTRIBUTES,PSECURITY_DESCRIPTOR,
            Authorization::{ConvertSidToStringSidW,ConvertStringSecurityDescriptorToSecurityDescriptorW}},
        System::{Threading::{OpenProcess,GetCurrentProcess,OpenProcessToken,QueryFullProcessImageNameW,PROCESS_NAME_FORMAT,PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_SYNCHRONIZE,WaitForSingleObject,GetExitCodeProcess},
            Pipes::{GetNamedPipeClientProcessId,GetNamedPipeServerProcessId},
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot,Process32FirstW,Process32NextW,PROCESSENTRY32W,TH32CS_SNAPPROCESS}}}};
    fn error(e:impl std::fmt::Display)->io::Error{io::Error::other(e.to_string())}
    struct Handle(HANDLE);impl Drop for Handle{fn drop(&mut self){unsafe{let _=CloseHandle(self.0);}}}
    // Kernel process/token/snapshot handles are not tied to their opening
    // thread. Ownership stays unique and CloseHandle runs exactly once.
    unsafe impl Send for Handle {}
    impl Handle {fn wait_for_exit(self)->io::Result<()> {unsafe {
        if WaitForSingleObject(self.0,20_000)!=WAIT_OBJECT_0{return Err(error("application owner has not exited"));}
        let mut code=0;GetExitCodeProcess(self.0,&mut code).map_err(error)?;
        if code!=0{return Err(error("application exited abnormally"));}Ok(())
    }}}
    struct Allocation(HLOCAL);impl Drop for Allocation{fn drop(&mut self){unsafe{let _=LocalFree(self.0);}}}
    fn sid(process:HANDLE)->io::Result<String>{unsafe{
        let mut token=HANDLE::default();OpenProcessToken(process,TOKEN_QUERY,&mut token).map_err(error)?;let _token=Handle(token);
        let mut size=0;let _=GetTokenInformation(token,TokenUser,None,0,&mut size);
        if size==0||size>16384{return Err(error("account identity unavailable"));}
        let mut bytes=vec![0usize;(size as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(token,TokenUser,Some(bytes.as_mut_ptr().cast()),size,&mut size).map_err(error)?;
        let user=&*bytes.as_ptr().cast::<TOKEN_USER>();let mut result=PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid,&mut result).map_err(error)?;let _allocation=Allocation(HLOCAL(result.0.cast()));
        result.to_string().map_err(error)
    }}
    fn image(process:HANDLE)->io::Result<PathBuf>{unsafe{
        let mut buf=vec![0u16;32768];let mut size=buf.len() as u32;
        QueryFullProcessImageNameW(process,PROCESS_NAME_FORMAT(0),PWSTR(buf.as_mut_ptr()),&mut size).map_err(error)?;
        Ok(PathBuf::from(String::from_utf16_lossy(&buf[..size as usize])))
    }}
    fn normalize(path:&Path)->io::Result<String>{Ok(path.canonicalize()?.to_string_lossy().trim_start_matches(r"\\?\").to_lowercase())}
    fn endpoint()->io::Result<String>{
        let identity=format!("{}|{}",sid(unsafe{GetCurrentProcess()})?,normalize(&std::env::current_exe()?)?);
        Ok(format!(r"\\.\pipe\SpeakMode.Maintenance.{}",hex_digest(identity.as_bytes())))
    }
    #[cfg(test)] pub(super) fn test_endpoint()->io::Result<String>{endpoint()}
    fn hex_digest(bytes:&[u8])->String{Sha256::digest(bytes).iter().map(|byte|format!("{byte:02x}")).collect()}
    fn owned_process(pid:u32)->io::Result<Handle>{unsafe{
        let handle=Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_SYNCHRONIZE,false,pid).map_err(error)?);
        if normalize(&image(handle.0)?)?!=normalize(&std::env::current_exe()?)?||sid(handle.0)?!=sid(GetCurrentProcess())?{return Err(error("maintenance peer identity mismatch"));}
        Ok(handle)
    }}
    fn any_owned_process()->io::Result<bool>{unsafe{
        let snapshot=Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS,0).map_err(error)?);
        let own_exe=std::env::current_exe()?;let name=own_exe.file_name().ok_or_else(||error("executable identity unavailable"))?.to_string_lossy().to_lowercase();
        let mut entry=PROCESSENTRY32W{dwSize:std::mem::size_of::<PROCESSENTRY32W>() as u32,..Default::default()};
        let mut present=Process32FirstW(snapshot.0,&mut entry).is_ok();
        while present {
            let count=entry.szExeFile.iter().position(|c|*c==0).unwrap_or(entry.szExeFile.len());
            if entry.th32ProcessID!=std::process::id()&&String::from_utf16_lossy(&entry.szExeFile[..count]).to_lowercase()==name {
                let process=Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,entry.th32ProcessID).map_err(error)?);
                if normalize(&image(process.0)?)?==normalize(&own_exe)?&&sid(process.0)?==sid(GetCurrentProcess())?{return Ok(true);}
            }
            present=Process32NextW(snapshot.0,&mut entry).is_ok();
        }
        Ok(false)
    }}
    fn create_pipe(name:&str,first:bool)->io::Result<tokio::net::windows::named_pipe::NamedPipeServer>{unsafe{
        // Only this account and SYSTEM. Reject remote clients independently.
        let sddl=format!("D:P(A;;GA;;;SY)(A;;GA;;;{})",sid(GetCurrentProcess())?);
        let wide:Vec<u16>=sddl.encode_utf16().chain(Some(0)).collect();let mut descriptor=PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(wide.as_ptr()),1,&mut descriptor,None).map_err(error)?;
        let _descriptor=Allocation(HLOCAL(descriptor.0));
        let mut attrs=SECURITY_ATTRIBUTES{nLength:std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:descriptor.0,bInheritHandle:false.into()};
        ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).max_instances(2)
            .create_with_security_attributes_raw(name,(&mut attrs as *mut SECURITY_ATTRIBUTES).cast())
    }}
    async fn restore_listener(name:&str,stopping:&CancellationToken)->io::Result<Option<tokio::net::windows::named_pipe::NamedPipeServer>> {
        let deadline=tokio::time::Instant::now()+Duration::from_secs(1);
        loop {
            if stopping.is_cancelled(){return Ok(None);}
            match create_pipe(name,false){
                Ok(pipe)=>return Ok(Some(pipe)),
                Err(error)if error.raw_os_error()==Some(231)&&tokio::time::Instant::now()<deadline=>{},
                Err(error)=>return Err(error),
            }
            tokio::select!{_ = stopping.cancelled()=>return Ok(None),_ = tokio::time::sleep(Duration::from_millis(10))=>{}}
        }
    }
    pub fn start_server(prepare:Prepare,on_ready:Arc<dyn Fn(VerdictDelivery)+Send+Sync>)->io::Result<Server>{
        let name=endpoint()?;let first=create_pipe(&name,true)?;
        let cancel=CancellationToken::new();let stopping=cancel.clone();let released=Arc::new(AtomicBool::new(false));let exited=Arc::clone(&released);
        struct Lease(Arc<AtomicBool>);impl Drop for Lease{fn drop(&mut self){self.0.store(true,Ordering::Release);}}
        tokio::spawn(async move{
            // Usually keep one pending listener. A retiring ACK client can
            // briefly occupy the other of the two kernel instances; serving
            // the already-connected request must not depend on creating a
            // third instance. Restore admission after releasing current, with
            // a bounded cancellation-aware wait for those old handles.
            let _lease=Lease(exited);let mut next=Some(first);
            loop {
                let mut pipe=match next.take(){Some(pipe)=>pipe,None=>match restore_listener(&name,&stopping).await{Ok(Some(pipe))=>pipe,_=>return}};
                let connected=tokio::select!{_ = stopping.cancelled()=>return,result=pipe.connect()=>result};
                if connected.is_err(){return;}
                next=match create_pipe(&name,false){Ok(pipe)=>Some(pipe),Err(error)if error.raw_os_error()==Some(231)=>None,Err(_)=>return};
                let mut pid=0;
                let authorized=unsafe{GetNamedPipeClientProcessId(HANDLE(pipe.as_raw_handle()),&mut pid)}.is_ok()&&owned_process(pid).is_ok();
                if !authorized{continue;}
                let mut request=vec![0u8;REQUEST.len()];
                let read=tokio::select!{_ = stopping.cancelled()=>return,result=tokio::time::timeout(Duration::from_secs(3),pipe.read_exact(&mut request))=>result};
                if !matches!(read,Ok(Ok(_)))||request!=REQUEST{continue;}
                let result=prepare().await;
                let response=if result.is_ok(){READY}else{FAILED};
                let written=tokio::select!{_ = stopping.cancelled()=>false,result=tokio::time::timeout(Duration::from_secs(2),async{pipe.write_all(response).await?;pipe.flush().await})=>matches!(result,Ok(Ok(())))};
                // DisconnectNamedPipe discards unread bytes. Exact ACK records
                // delivery; absent/wrong ACK never masquerades as confirmed.
                // After actual resource cleanup, communication failure still
                // permits ordinary exit. The helper separately requires READY
                // plus the process's real normal exit before deleting anything.
                let mut ack=[0u8;4];
                let read=tokio::select!{_ = stopping.cancelled()=>false,result=tokio::time::timeout(Duration::from_secs(3),pipe.read_exact(&mut ack))=>matches!(result,Ok(Ok(_)))};
                let delivery=if written&&read&&ack==ACK{VerdictDelivery::Acknowledged}else{VerdictDelivery::Unconfirmed};
                if result.is_ok(){on_ready(delivery);return;}
                if stopping.is_cancelled(){return;}
            }
        });
        Ok(Server{cancel,released})
    }
    pub async fn prepare_uninstall()->io::Result<()> {
        let name=endpoint()?;let deadline=tokio::time::Instant::now()+Duration::from_secs(3);
        let mut pipe=loop{match ClientOptions::new().open(&name){
            Ok(pipe)=>break pipe,
            Err(e)if matches!(e.raw_os_error(),Some(2|231))=>{
                if !any_owned_process()?{return Ok(());}
                if tokio::time::Instant::now()>=deadline{return Err(error("running application did not accept maintenance"));}
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e)=>return Err(e),
        }};
        let mut pid=0;unsafe{GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()),&mut pid).map_err(error)?;}
        let process=owned_process(pid)?;
        tokio::time::timeout(Duration::from_secs(25),async{
            pipe.write_all(REQUEST).await?;pipe.flush().await?;let mut response=[0u8;6];pipe.read_exact(&mut response).await?;
            pipe.write_all(ACK).await?;pipe.flush().await?;
            if response.as_slice()!=READY{return Err(error("application cleanup was not confirmed"));}Ok::<_,io::Error>(())
        }).await.map_err(|_|error("maintenance response deadline exceeded"))??;
        drop(pipe);
        let exited=tokio::task::spawn_blocking(move||process.wait_for_exit()).await.map_err(error)?;
        exited?;
        if any_owned_process()?{return Err(error("another application owner is still running"));}
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    static NATIVE_TRANSPORT_TEST_LOCK:std::sync::Mutex<()>=std::sync::Mutex::new(());
    fn test_scope(scope:&str)->PathBuf{
        assert!(matches!(scope,"maintenance-data"|"maintenance-native"));
        let manifest=PathBuf::from(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap();
        let path=manifest.join(".test-data").join(scope);
        std::fs::create_dir_all(&path).unwrap();
        assert_eq!(path.canonicalize().unwrap(),path,"fixture scope must stay in the source tree");
        path
    }
    fn validate_test_fixture(root:&Path,scope:&str)->PathBuf{
        assert!(root.is_absolute(),"controlled fixture must be absolute");
        assert!(!root.components().any(|part|matches!(part,std::path::Component::ParentDir)));
        assert!(!is_reparse(&std::fs::symlink_metadata(root).unwrap()),"fixture may not be redirected");
        let resolved=root.canonicalize().unwrap();
        assert_eq!(resolved,root,"fixture must use its canonical path");
        let expected_scope=test_scope(scope);
        assert_eq!(resolved.parent(),Some(expected_scope.as_path()),"fixture must be a direct child of the controlled scope");
        let name=resolved.file_name().and_then(|name|name.to_str()).expect("fixture name");
        assert!(uuid::Uuid::parse_str(name).is_ok(),"fixture must use a UUID");
        resolved
    }
    fn test_fixture(scope:&str)->PathBuf{
        let root=test_scope(scope).join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&root).unwrap();
        validate_test_fixture(&root,scope)
    }
    #[test]fn maintenance_arguments_are_explicit_and_do_not_fall_through_to_gui(){
        assert_eq!(parse_arguments(vec!["--autostart".into()]).unwrap(),None);
        assert!(parse_arguments(vec!["--delete-user-data".into()]).is_err());
        assert!(parse_arguments(vec!["--prepare-uninstall".into(),"--record".into()]).is_err());
        assert_eq!(parse_arguments(vec!["--prepare-uninstall".into()]).unwrap(),Some(Options::default()));
        assert_eq!(parse_arguments(vec!["--prepare-uninstall".into(),"--delete-cloud-key".into()]).unwrap(),Some(Options{delete_data:false,delete_key:true}));
    }
    #[test]fn data_cleanup_refuses_unmarked_or_other_root_and_preserves_siblings(){
        let base=test_fixture("maintenance-data");
        let owned=base.join("owned");let sibling=base.join("sibling");std::fs::create_dir_all(&owned).unwrap();std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(owned.join("speakmode.sqlite"),b"controlled test data").unwrap();
        std::fs::write(sibling.join("keep.txt"),b"keep").unwrap();
        assert!(delete_data(&owned,&owned).is_err());assert!(owned.exists());
        mark_profile(&owned).unwrap();assert!(delete_data(&owned,&sibling).is_err());assert!(owned.exists());
        delete_data(&owned,&owned).unwrap();assert!(!owned.exists());assert!(sibling.join("keep.txt").exists());
        std::fs::remove_dir_all(base).unwrap();
    }
    // Run only by the parent below, with a canonical UUID fixture. No app,
    // WebView, microphone, cloud configuration or real profile is opened.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn native_server_child(){
        let requested=PathBuf::from(std::env::var_os("SPEAKMODE_MAINTENANCE_TEST_ROOT").expect("controlled test directory"));
        let root=validate_test_fixture(&requested,"maintenance-native");
        let runtime=tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let requests=Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let prepare:Prepare=Arc::new({let root=root.clone();move||{
                let root=root.clone();let requests=Arc::clone(&requests);
                Box::pin(async move {
                    let request=requests.fetch_add(1,Ordering::AcqRel);
                    if request==0{return Err(io::Error::other("controlled resource still held"));}
                    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    std::fs::write(root.join("resources-released"),b"capture/db/hook owners released")?;
                    Ok(())
                })
            }});
            let server=start_server(prepare,Arc::new({let root=root.clone();move|delivery|{
                std::fs::write(root.join("verdict-delivery"),format!("{delivery:?}")).unwrap();std::process::exit(0)
            }})).unwrap();
            std::fs::write(root.join("server-ready"),b"ready").unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            server.stop();
            panic!("parent did not complete controlled maintenance");
        });
    }
    #[cfg(windows)]
    #[test]
    fn native_transport_requires_protocol_cleanup_and_real_process_exit(){
        let _native_test=NATIVE_TRANSPORT_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::{os::windows::process::CommandExt,process::{Command,Stdio},time::{Duration,Instant}};
        use tokio::{io::AsyncWriteExt,net::windows::named_pipe::ClientOptions};
        let root=test_fixture("maintenance-native");
        std::fs::create_dir_all(&root).unwrap();
        let output=std::fs::File::create(root.join("child.log")).unwrap();
        let mut child=Command::new(std::env::current_exe().unwrap())
            .args(["--exact","maintenance::tests::native_server_child","--ignored","--nocapture"])
            .env("SPEAKMODE_MAINTENANCE_TEST_ROOT",&root).stdout(Stdio::from(output.try_clone().unwrap()))
            .stderr(Stdio::from(output)).creation_flags(0x08000000).spawn().unwrap();
        let deadline=Instant::now()+Duration::from_secs(8);
        while !root.join("server-ready").exists(){
            assert!(child.try_wait().unwrap().is_none(),"controlled owner exited early");
            assert!(Instant::now()<deadline,"controlled server startup deadline");
            std::thread::sleep(Duration::from_millis(20));
        }
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut wrong=ClientOptions::new().open(native::test_endpoint().unwrap()).unwrap();
            wrong.write_all(&vec![b'x';REQUEST.len()]).await.unwrap();drop(wrong);
            tokio::time::sleep(Duration::from_millis(40)).await;
            let failure=prepare_uninstall().await.unwrap_err();
            assert!(failure.to_string().contains("cleanup was not confirmed"),"cleanup failure must refuse uninstall: {failure}");
            assert!(child.try_wait().unwrap().is_none(),"failure must keep owner alive");
            assert!(!root.join("resources-released").exists(),"invalid protocol must not run cleanup");
            prepare_uninstall().await.unwrap();
            assert!(root.join("resources-released").exists(),"READY must follow resource acknowledgement");
            assert!(child.try_wait().unwrap().unwrap().success(),"helper must confirm actual normal owner exit");
            assert_eq!(std::fs::read_to_string(root.join("verdict-delivery")).unwrap(),"Acknowledged");
            let server=start_server(Arc::new(||Box::pin(async {Ok(())})),Arc::new(|_|{})).unwrap();
            assert!(!server.released());server.stop();
            tokio::time::timeout(Duration::from_secs(2),async{while !server.released(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
            assert!(server.released(),"server lease follows task owner exit");
            // A connected local client must not keep normal exit waiting for
            // its REQUEST or ACK deadline after the server is told to stop.
            for wait_at_ack in [false,true]{
                let server=start_server(Arc::new(||Box::pin(async {Err(io::Error::other("controlled cleanup refusal"))})),Arc::new(|_|{})).unwrap();
                let mut slow=ClientOptions::new().open(native::test_endpoint().unwrap()).unwrap();
                if wait_at_ack{
                    use tokio::io::AsyncReadExt;
                    slow.write_all(REQUEST).await.unwrap();let mut reply=[0u8;6];slow.read_exact(&mut reply).await.unwrap();assert_eq!(&reply,FAILED);
                }else{tokio::time::sleep(Duration::from_millis(30)).await;}
                server.stop();
                tokio::time::timeout(Duration::from_millis(500),async{while !server.released(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
                drop(slow);
            }
        });
        std::fs::remove_dir_all(root).unwrap();
        for wrong_ack in [true,false]{
            let root=test_fixture("maintenance-native");
            std::fs::create_dir_all(&root).unwrap();
            let output=std::fs::File::create(root.join("child.log")).unwrap();
            let mut child=Command::new(std::env::current_exe().unwrap())
                .args(["--exact","maintenance::tests::native_server_child","--ignored","--nocapture"])
                .env("SPEAKMODE_MAINTENANCE_TEST_ROOT",&root).stdout(Stdio::from(output.try_clone().unwrap()))
                .stderr(Stdio::from(output)).creation_flags(0x08000000).spawn().unwrap();
            let deadline=Instant::now()+Duration::from_secs(8);
            while !root.join("server-ready").exists(){assert!(child.try_wait().unwrap().is_none());assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(20));}
            runtime.block_on(async{
                use tokio::io::AsyncReadExt;
                assert!(prepare_uninstall().await.is_err());
                let mut raw=ClientOptions::new().open(native::test_endpoint().unwrap()).unwrap();raw.write_all(REQUEST).await.unwrap();
                let mut reply=[0u8;6];raw.read_exact(&mut reply).await.unwrap();assert_eq!(&reply,READY);
                if wrong_ack{raw.write_all(b"BAD\n").await.unwrap();}drop(raw);
            });
            let deadline=Instant::now()+Duration::from_secs(5);
            while child.try_wait().unwrap().is_none(){assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(20));}
            assert!(child.wait().unwrap().success());assert!(root.join("resources-released").exists());
            assert_eq!(std::fs::read_to_string(root.join("verdict-delivery")).unwrap(),"Unconfirmed");
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[cfg(windows)]
    #[test]
    fn native_transport_retiring_ack_client_does_not_discard_next_protocol_connection(){
        let _native_test=NATIVE_TRANSPORT_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::time::Duration;
        use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::windows::named_pipe::ClientOptions};
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
            let requests=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=requests.clone();
            let server=start_server(Arc::new(move||{observed.fetch_add(1,Ordering::SeqCst);Box::pin(async{Err(io::Error::other("controlled cleanup refusal"))})}),Arc::new(|_|panic!("failed cleanup must not issue READY"))).unwrap();
            let mut retiring=ClientOptions::new().open(native::test_endpoint().unwrap()).unwrap();
            retiring.write_all(REQUEST).await.unwrap();let mut first=[0u8;6];retiring.read_exact(&mut first).await.unwrap();assert_eq!(&first,FAILED);
            retiring.write_all(ACK).await.unwrap();retiring.flush().await.unwrap();
            // Keep the acknowledged client's kernel handle alive. It still
            // occupies one instance while the already-created listener accepts
            // the next owned client, so a third instance cannot yet be created.
            let mut next=ClientOptions::new().open(native::test_endpoint().unwrap()).unwrap();next.write_all(REQUEST).await.unwrap();
            let mut second=[0u8;6];let response=tokio::time::timeout(Duration::from_secs(1),next.read_exact(&mut second)).await;
            drop(retiring);drop(next);server.stop();
            tokio::time::timeout(Duration::from_millis(500),async{while !server.released(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
            assert!(matches!(response,Ok(Ok(_))),"an acknowledged retiring client must not discard the next protocol connection: {response:?}");
            assert_eq!(&second,FAILED);assert_eq!(requests.load(Ordering::SeqCst),2);
        });
    }
    #[cfg(windows)]
    #[test]
    fn native_transport_listener_recovery_can_be_cancelled_while_ack_clients_hold_capacity(){
        let _native_test=NATIVE_TRANSPORT_TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        use std::time::Duration;
        use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::windows::named_pipe::ClientOptions};
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
            let requests=Arc::new(std::sync::atomic::AtomicUsize::new(0));let observed=requests.clone();
            let server=start_server(Arc::new(move||{observed.fetch_add(1,Ordering::SeqCst);Box::pin(async{Err(io::Error::other("controlled cleanup refusal"))})}),Arc::new(|_|panic!("failed cleanup must not issue READY"))).unwrap();
            let mut held=Vec::new();
            for _ in 0..2{
                let mut client=ClientOptions::new().open(native::test_endpoint().unwrap()).unwrap();client.write_all(REQUEST).await.unwrap();
                let mut reply=[0u8;6];tokio::time::timeout(Duration::from_secs(1),client.read_exact(&mut reply)).await.unwrap().unwrap();assert_eq!(&reply,FAILED);
                client.write_all(ACK).await.unwrap();client.flush().await.unwrap();held.push(client);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;assert!(!server.released());
            server.stop();
            tokio::time::timeout(Duration::from_millis(500),async{while !server.released(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
            assert_eq!(requests.load(Ordering::SeqCst),2);drop(held);
        });
    }
    #[cfg(windows)]
    #[test]
    fn locked_data_cleanup_keeps_marker_for_retry(){
        use std::os::windows::fs::OpenOptionsExt;
        let root=test_fixture("maintenance-data");
        std::fs::create_dir_all(&root).unwrap();mark_profile(&root).unwrap();
        let held=std::fs::OpenOptions::new().create_new(true).write(true).share_mode(1).open(root.join("held.webview-file")).unwrap();
        assert!(delete_data(&root,&root).is_err());assert!(root.join(PROFILE_MARKER).exists());
        drop(held);delete_data(&root,&root).unwrap();assert!(!root.exists());
    }
}

#[cfg(target_os="macos")]
#[path="platform/macos/maintenance.rs"]
mod macos;
#[cfg(target_os="macos")]
pub use macos::{start_server,prepare_uninstall};

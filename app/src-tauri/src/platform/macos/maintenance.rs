//! Local authenticated maintenance transport. Only the same user's exact
//! executable can request cleanup. Socket permissions never replace peer checks.
use super::*;
use crate::platform::macos::process_info;
use std::{os::unix::{fs::{MetadataExt, PermissionsExt, FileTypeExt}, ffi::OsStrExt}, time::Duration};
use tokio::{net::{UnixListener, UnixStream}, io::{AsyncReadExt, AsyncWriteExt}};
use sha2::{Digest, Sha256};
fn error(message: &'static str) -> io::Error { io::Error::other(message) }
fn image(pid: u32) -> io::Result<PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length = unsafe { libc::proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 { return Err(error("process identity unavailable")); }
    let length = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
    Path::new(std::ffi::OsStr::from_bytes(&buffer[..length])).canonicalize()
}
fn owned_process(pid: u32) -> io::Result<u64> {
    let (uid, lifetime) = process_info(pid).ok_or_else(|| error("process identity unavailable"))?;
    if uid != unsafe { libc::geteuid() } || image(pid)? != std::env::current_exe()?.canonicalize()? { return Err(error("maintenance peer identity mismatch")); }
    Ok(lifetime)
}
fn any_owned_process() -> io::Result<bool> {
    let mut pids = vec![0i32; 65536];
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * std::mem::size_of::<i32>()) as i32) };
    if count < 0 || count as usize >= pids.len() { return Err(error("process enumeration unavailable")); }
    let own = std::process::id();
    let own_image = std::env::current_exe()?.canonicalize()?;
    for pid in pids.into_iter().take(count as usize).filter(|pid| *pid > 0 && *pid as u32 != own) {
        if process_info(pid as u32).is_some_and(|(uid,_)| uid == unsafe { libc::geteuid() }) && image(pid as u32).is_ok_and(|path| path == own_image) { return Ok(true); }
    }
    Ok(false)
}
fn endpoint(create: bool) -> io::Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let directory = PathBuf::from(format!("/private/tmp/speakmode-maintenance-{uid}"));
    if create && !directory.exists() {
        use std::os::unix::fs::DirBuilderExt;
        match std::fs::DirBuilder::new().mode(0o700).create(&directory) { Ok(()) => {}, Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}, Err(e) => return Err(e) }
    }
    if directory.exists() {
        let meta = std::fs::symlink_metadata(&directory)?;
        if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 { return Err(error("maintenance directory ownership mismatch")); }
    }
    let hash = format!("{:x}", Sha256::digest(std::env::current_exe()?.canonicalize()?.as_os_str().as_bytes()));
    Ok(directory.join(format!("{}.sock", &hash[..24])))
}
fn peer(stream: &UnixStream) -> io::Result<(u32, u64)> {
    let credentials = stream.peer_cred()?;
    if credentials.uid() != unsafe { libc::geteuid() } { return Err(error("maintenance account mismatch")); }
    let pid = credentials.pid().filter(|pid| *pid > 0).ok_or_else(|| error("maintenance peer pid unavailable"))? as u32;
    Ok((pid, owned_process(pid)?))
}
pub fn start_server(prepare: Prepare, on_ready: Arc<dyn Fn(VerdictDelivery) + Send + Sync>) -> io::Result<Server> {
    start_at(endpoint(true)?, prepare, on_ready)
}
fn start_at(path: PathBuf, prepare: Prepare, on_ready: Arc<dyn Fn(VerdictDelivery) + Send + Sync>) -> io::Result<Server> {
    if path.exists() {
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } || any_owned_process()? { return Err(error("maintenance endpoint already owned")); }
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let cancel = CancellationToken::new();
    let stopping = cancel.clone();
    let released = Arc::new(AtomicBool::new(false));
    let exited = released.clone();
    tokio::spawn(async move {
        struct Lease { path: PathBuf, released: Arc<AtomicBool> }
        impl Drop for Lease { fn drop(&mut self) { let _ = std::fs::remove_file(&self.path); self.released.store(true, Ordering::Release); } }
        let _lease = Lease { path, released: exited };
        loop {
            let accepted = tokio::select! { _ = stopping.cancelled() => return, value = listener.accept() => value };
            let Ok((mut stream,_)) = accepted else { return; };
            if peer(&stream).is_err() { continue; }
            let mut request = vec![0; REQUEST.len()];
            let read = tokio::select! { _ = stopping.cancelled() => return, result = tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut request)) => result };
            if !matches!(read, Ok(Ok(_))) || request != REQUEST { continue; }
            let result = prepare().await;
            let written = tokio::time::timeout(Duration::from_secs(2), stream.write_all(if result.is_ok() { READY } else { FAILED })).await;
            let mut ack = [0u8; 4];
            let received = tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut ack)).await;
            if result.is_ok() {
                on_ready(if matches!(written, Ok(Ok(()))) && matches!(received, Ok(Ok(_))) && ack == ACK { VerdictDelivery::Acknowledged } else { VerdictDelivery::Unconfirmed });
                return;
            }
        }
    });
    Ok(Server { cancel, released })
}
pub async fn prepare_uninstall() -> io::Result<()> {
    let path = endpoint(false)?;
    let mut stream = match UnixStream::connect(path).await {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) && !any_owned_process()? => return Ok(()),
        Err(e) => return Err(e),
    };
    let (pid,lifetime) = peer(&stream)?;
    tokio::time::timeout(Duration::from_secs(25), async {
        stream.write_all(REQUEST).await?;
        let mut response = [0; 6]; stream.read_exact(&mut response).await?;
        stream.write_all(ACK).await?;
        if response.as_slice() != READY { return Err(error("application cleanup was not confirmed")); }
        Ok::<_, io::Error>(())
    }).await.map_err(|_| error("maintenance deadline exceeded"))??;
    drop(stream);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while process_info(pid).is_some_and(|(_,start)| start == lifetime) {
        if tokio::time::Instant::now() >= deadline { return Err(error("application did not exit after cleanup")); }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    if any_owned_process()? { return Err(error("another application owner is still running")); }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn peer_identity_is_the_same_executable_and_process_lifetime() {
        assert_eq!(owned_process(std::process::id()).unwrap(), crate::platform::macos::process_lifetime(std::process::id()).unwrap());
        assert!(owned_process(0).is_err());
    }
    #[test] fn endpoint_is_short_and_specific_to_the_current_executable() {
        let path = endpoint(false).unwrap();
        assert!(path.as_os_str().as_bytes().len() < 104);
        assert_eq!(path.extension().unwrap(), "sock");
    }
    #[tokio::test]
    async fn unix_transport_confirms_cleanup_and_releases_its_listener() {
        let path = PathBuf::from(format!("/private/tmp/sm-maint-test-{}.sock", uuid::Uuid::new_v4()));
        let ready = Arc::new(AtomicBool::new(false));
        let callback = ready.clone();
        let server = start_at(path.clone(), Arc::new(|| Box::pin(async { Ok(()) })), Arc::new(move |delivery| {
            assert!(matches!(delivery, VerdictDelivery::Acknowledged)); callback.store(true, Ordering::Release);
        })).unwrap();
        let mut stream = UnixStream::connect(&path).await.unwrap();
        assert_eq!(peer(&stream).unwrap().0, std::process::id());
        stream.write_all(REQUEST).await.unwrap();
        let mut response = [0;6]; stream.read_exact(&mut response).await.unwrap(); assert_eq!(&response, READY);
        stream.write_all(ACK).await.unwrap(); drop(stream);
        tokio::time::timeout(Duration::from_secs(2), async { while !server.released() { tokio::task::yield_now().await; } }).await.unwrap();
        assert!(ready.load(Ordering::Acquire)); assert!(!path.exists());
    }

}

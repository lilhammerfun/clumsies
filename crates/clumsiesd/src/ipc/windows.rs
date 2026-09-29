//! Local, current-user-only named pipes. The JSON contract is shared with XPC/Unix.
use super::*;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_PIPE_BUSY, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

const MAX_FRAME: usize = 64 * 1024 * 1024;

fn io_error(error: impl std::fmt::Display) -> DaemonError {
    DaemonError::Ipc(error.to_string())
}

fn current_sid() -> Result<String, DaemonError> {
    // Token buffers use usize elements to preserve TOKEN_USER's pointer alignment.
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        let mut bytes = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut bytes);
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        );
        let error = std::io::Error::last_os_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(io_error(error));
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut sid = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        let mut length = 0;
        while *sid.add(length) != 0 {
            length += 1;
        }
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(sid, length));
        LocalFree(sid.cast());
        Ok(result)
    }
}

fn endpoint(service: &str) -> Result<String, DaemonError> {
    let root = crate::config::daemon_root_dir()?;
    let identity = format!("{}:{}:{}", current_sid()?, root.display(), service);
    Ok(format!(
        r"\\.\pipe\clumsies-{:x}",
        Sha256::digest(identity.as_bytes())
    ))
}

fn listener(name: &str, first: bool) -> Result<NamedPipeServer, DaemonError> {
    let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", current_sid()?)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let result = ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                (&attributes as *const SECURITY_ATTRIBUTES)
                    .cast_mut()
                    .cast(),
            );
        LocalFree(descriptor);
        result.map_err(io_error)
    }
}

async fn read_frame(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Result<String, DaemonError> {
    let length = stream.read_u32().await.map_err(io_error)? as usize;
    if length > MAX_FRAME {
        return Err(io_error("IPC frame exceeds 64 MiB"));
    }
    let mut data = vec![0; length];
    stream.read_exact(&mut data).await.map_err(io_error)?;
    String::from_utf8(data).map_err(io_error)
}

async fn write_frame(
    stream: &mut (impl tokio::io::AsyncWrite + Unpin),
    data: &str,
) -> Result<(), DaemonError> {
    if data.len() > MAX_FRAME {
        return Err(io_error("IPC frame exceeds 64 MiB"));
    }
    stream
        .write_u32(data.len() as u32)
        .await
        .map_err(io_error)?;
    stream.write_all(data.as_bytes()).await.map_err(io_error)?;
    stream.flush().await.map_err(io_error)
}

pub fn call(
    service: &str,
    request: DaemonIpcRequest,
    timeout: Duration,
) -> Result<DaemonIpcResponse, DaemonError> {
    let name = endpoint(service)?;
    // A synchronous API may be called from an existing Tokio runtime. A separate
    // thread avoids nesting block_on and owns the timeout/cancellation of its I/O.
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(io_error)?;
        runtime.block_on(async {
            tokio::time::timeout(timeout, async {
                let mut pipe = loop {
                    match ClientOptions::new().open(&name) {
                        Ok(pipe) => break pipe,
                        Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        Err(error) => return Err(io_error(error)),
                    }
                };
                write_frame(&mut pipe, &serde_json::to_string(&request)?).await?;
                serde_json::from_str(&read_frame(&mut pipe).await?).map_err(DaemonError::from)
            })
            .await
            .map_err(|_| io_error("daemon IPC timed out"))?
        })
    })
    .join()
    .map_err(|_| io_error("daemon IPC worker stopped"))?
}

pub struct DaemonIpcServerInner {
    service_name: String,
    task: tokio::task::JoinHandle<()>,
}

impl DaemonIpcServerInner {
    pub fn start(service_name: String, service: DaemonIpcService) -> Result<Self, DaemonError> {
        let name = endpoint(&service_name)?;
        let mut pipe = listener(&name, true)?;
        let task = tokio::runtime::Handle::try_current()
            .map_err(io_error)?
            .spawn(async move {
                loop {
                    if pipe.connect().await.is_err() {
                        break;
                    }
                    // Keep a listening instance available before handing off the connected one.
                    let next = match listener(&name, false) {
                        Ok(next) => next,
                        Err(_) => break,
                    };
                    let mut connected = std::mem::replace(&mut pipe, next);
                    let service = service.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(90), async {
                            let request = read_frame(&mut connected).await;
                            if let Ok(reply) = dispatch_request(&service, request).await {
                                let _ = write_frame(&mut connected, &reply).await;
                            }
                        })
                        .await;
                    });
                }
            });
        Ok(Self { service_name, task })
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }
}

impl Drop for DaemonIpcServerInner {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pipe_is_exclusive_local_and_transfers_bounded_frames() {
        let name = endpoint(&format!("test-{}", uuid::Uuid::new_v4())).unwrap();
        let server = listener(&name, true).unwrap();
        assert!(listener(&name, true).is_err());
        let task = tokio::spawn(async move {
            server.connect().await.unwrap();
            let mut server = server;
            assert_eq!(read_frame(&mut server).await.unwrap(), "hello");
            write_frame(&mut server, "reply").await.unwrap();
        });
        let mut client = ClientOptions::new().open(&name).unwrap();
        write_frame(&mut client, "hello").await.unwrap();
        assert_eq!(read_frame(&mut client).await.unwrap(), "reply");
        task.await.unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(8);
        writer.write_u32(MAX_FRAME as u32 + 1).await.unwrap();
        assert!(read_frame(&mut reader).await.is_err());
    }
}

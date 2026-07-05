#![windows_subsystem = "windows"]

#[macro_use]
extern crate tracing;

use std::{
    fmt::Debug,
    future::Future,
    io::ErrorKind,
    mem::{self, MaybeUninit},
    os::windows::io::AsRawHandle,
    pin::{pin, Pin},
    sync::{Arc, Barrier},
    thread::{self, JoinHandle},
    time::Duration,
};

use clap::Parser;
use erratic::*;
use futures::{channel::oneshot, FutureExt};
use regex::Regex;
use tokio::{
    io::AsyncReadExt,
    net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions},
};
use tokio_util::sync::CancellationToken;
use tracing::Level;
use windows::Win32::{
    Foundation::{BOOL, HANDLE, HWND, LPARAM, WPARAM},
    System::Threading::GetThreadId,
    UI::{
        Input::KeyboardAndMouse::{
            MapVirtualKeyW, RegisterHotKey, UnregisterHotKey, MAPVK_VK_TO_VSC, MOD_CONTROL,
            VK_OEM_3,
        },
        WindowsAndMessaging::{
            DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowTextW, PeekMessageW,
            PostMessageA, PostQuitMessage, PostThreadMessageW, TranslateMessage, MSG, PM_NOREMOVE,
            WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
        },
    },
};

const WM_APP_SHUTDOWN: u32 = WM_APP + 2;
const KID_CTRLOEM3: usize = 2333;
const PIPE_SERVER: &str = r"\\.\pipe\vscode.ext.ctrl-oem3";

const REQUEST_SHUTDOWN: u8 = 72;

#[derive(Parser)]
struct Cli {
    #[arg(long, value_parser = Self::decode_base64)]
    matches_window_title: String,
}

impl Cli {
    fn decode_base64(encoded: &str) -> Result<String> {
        use base64::prelude::*;

        Ok(String::from_utf8(BASE64_STANDARD.decode(encoded)?)?)
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(Level::INFO)
        .init();

    let log_path = std::env::temp_dir().join("vscode.ext.ctrl-oem3.fatal.log");

    info!("ctrl-oem3 service started. ");
    info!(
        "file logging is enabled for fatal-level messages: {}",
        log_path.display()
    );

    let Err(err) = main_() else {
        info!("ctrl-oem3 service stopped. ");
        return;
    };

    eprintln!("vscode.ext.ctrl-oem3.fatal: {err:?}");

    if let Err(write_err) = std::fs::write(&log_path, format!("{err:?}")) {
        eprintln!(
            "vscode.ext.ctrl-oem3.fatal: also failed to write log to {}: {write_err}",
            log_path.display()
        );
    }
}

fn main_() -> Result<()> {
    let cli = Cli::try_parse()?;

    info!("compiling pattern: {}", cli.matches_window_title);
    let re = Regex::new(&cli.matches_window_title).with_context(mkctx!(
        "failed to compile pattern: {}",
        cli.matches_window_title
    ))?;

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut fut_forward_hotkey = pin!(forward_hotkey(re));
            let mut fut_keepalive_server = pin!(keepalive_server());

            tokio::select! {
                res = &mut fut_forward_hotkey => res,
                res = &mut fut_keepalive_server =>res,
            }
        })?;

    Ok(())
}

fn forward_hotkey(matches_regex: Regex) -> ForwardHotkey {
    let (tx, rx) = oneshot::channel();
    let barrier = Arc::new(Barrier::new(2));
    let barrier2 = barrier.clone();
    let jh = thread::spawn(move || {
        // Force the thread to have a message queue: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-postthreadmessagew#remarks
        let mut msg: MSG = unsafe { mem::zeroed() };
        #[allow(unused_must_use)]
        unsafe {
            PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        }
        barrier2.wait();

        let res = forward_hotkey_sync(&matches_regex);
        if let Err(err) = tx.send(res) {
            error!("unable to send hotkey forwarding result: {err:?}");
        };
    });
    barrier.wait();

    ForwardHotkey {
        rx,
        jh: MaybeUninit::new(jh),
    }
}

#[derive(Debug)]
struct ForwardHotkey {
    rx: oneshot::Receiver<Result<()>>,
    jh: MaybeUninit<JoinHandle<()>>,
}

impl ForwardHotkey {
    fn notify_quit(&mut self) -> Result<()> {
        let handle = unsafe { self.jh.assume_init_ref().as_raw_handle() } as isize;
        let tid = unsafe { GetThreadId(HANDLE(handle)) };
        if tid == 0 {
            warn!("failed to get thread id, handle={handle:x} tid={tid:x}. ");
        } else {
            unsafe {
                PostThreadMessageW(tid, WM_APP_SHUTDOWN, None, None)
                    .map_err(|err| mkerr!("failed to post WM_APP_SHUTDOWN: {err}"))
                    .log_as_warning();
            }
        }

        Ok(())
    }
}

impl Future for ForwardHotkey {
    type Output = Result<()>;
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.rx
            .poll_unpin(cx)
            .map(|v| v.unwrap_or_else(|e| Err(e.into())))
    }
}

impl Drop for ForwardHotkey {
    fn drop(&mut self) {
        self.notify_quit().log_as_warning();
        unsafe {
            self.jh
                .assume_init_read()
                .join()
                .map_err(|_| mkerr!("failed to wait hotkey thread to exit. "))
                .log_as_warning();
        }
    }
}

fn forward_hotkey_sync(matches_regex: &Regex) -> Result<()> {
    unsafe { RegisterHotKey(None, KID_CTRLOEM3 as i32, MOD_CONTROL, VK_OEM_3.0 as _) }
        .with_context("failed to register hotkey")?;

    let mut msg: MSG = unsafe { mem::zeroed() };
    loop {
        let hr = unsafe { GetMessageW(&mut msg, HWND(0), 0, 0) };
        if matches!(hr, BOOL(0 | -1)) {
            break;
        }

        #[allow(unused_must_use)]
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        match msg.message {
            WM_HOTKEY if matches!(msg.wParam, WPARAM(KID_CTRLOEM3)) => {
                try_mimic_ctrl_oem3(matches_regex);
            }
            WM_APP_SHUTDOWN => unsafe {
                PostQuitMessage(0);
            },
            _ => {}
        }
    }

    unsafe {
        UnregisterHotKey(None, KID_CTRLOEM3 as i32).ok();
    }
    Ok(())
}

fn try_mimic_ctrl_oem3(matches_regex: &Regex) {
    unsafe {
        let hwnd = GetForegroundWindow();
        if matches!(hwnd, HWND(0)) {
            return;
        }

        let window_title = {
            let mut buffer = [0u16; 512];
            let buffer_used_count = GetWindowTextW(hwnd, &mut buffer) as usize;
            String::from_utf16_lossy(&buffer[..buffer_used_count])
        };
        if !matches_regex.is_match(&window_title) {
            return;
        }

        let scan_code = MapVirtualKeyW(VK_OEM_3.0 as _, MAPVK_VK_TO_VSC) as isize;
        let lparam_down = LPARAM(1 | (scan_code << 16));
        let lparam_up = LPARAM(1 | (scan_code << 16) | (1 << 30) | (1 << 31));

        PostMessageA(hwnd, WM_KEYDOWN, WPARAM(VK_OEM_3.0 as usize), lparam_down)
            .with_context("failed to mimic ctrl+oem3 to current window (keydown)")
            .log_as_warning();
        PostMessageA(hwnd, WM_KEYUP, WPARAM(VK_OEM_3.0 as usize), lparam_up)
            .with_context("failed to mimic ctrl+oem3 to current window (keyup)")
            .log_as_warning();
    }
}

async fn keepalive_server() -> Result<()> {
    if ClientOptions::new().open(PIPE_SERVER).is_ok() {
        info!("ctrl-oem3 service is already running, exit in favor of the existing one. ");
        return Ok(());
    }

    let (mut wit, mut waiter) = idle::new_pair(Duration::from_secs(30));
    let mut idle = pin!(waiter.wait());
    let token = CancellationToken::new();
    let mut cancelled = pin!(token.cancelled());

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(PIPE_SERVER)
        .with_context("failed to create named pipe server")?;

    loop {
        tokio::select! {
            res = server.connect() => res?,
            _ = &mut idle => break,
            _ = &mut cancelled => break,
        };

        let conn = server;
        server = ServerOptions::new().create(PIPE_SERVER)?;

        tokio::spawn(handle_connection(conn, wit.get_guard(), token.clone()));
    }

    Ok(())
}

async fn handle_connection(
    mut conn: NamedPipeServer,
    guard: idle::KeepaliveGuard,
    token: CancellationToken,
) {
    async move {
        let _guard = guard;
        let mut cancelled = pin!(token.cancelled());

        loop {
            let mut request = [0u8];

            tokio::select! {
                _ = &mut cancelled => break,
                res = conn.read_exact(&mut request) => match res {
                    Ok(_) => (),
                    Err(err) if matches!(err.kind(), ErrorKind::UnexpectedEof | ErrorKind::TimedOut) => break,
                    Err(err) => {
                        return mkres!(error = err, "failed to read from named pipe")
                    },
                },
            }

            match request {
                [REQUEST_SHUTDOWN] => {
                    token.cancel();
                    break
                }
                _ => return mkres!("received unknown request: {request:?}"),
            }
        }

        Ok(())
    }
    .await
    .log_as_warning();
}

mod idle {
    use std::{
        result,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use futures::TryFutureExt;
    use thiserror::Error;
    use tokio::{sync::watch, time};

    #[derive(Debug, Clone, Copy)]
    enum State {
        Active,
        Idle,
    }

    #[derive(Debug, Error)]
    pub enum Error {
        #[error("the paired `Witness` is dropped. ")]
        Disconnected,
    }

    type Result<T> = result::Result<T, Error>;

    pub struct Witness {
        active_count: Arc<AtomicUsize>,
        state: watch::Sender<State>,
    }

    impl Witness {
        pub fn get_guard(&mut self) -> KeepaliveGuard {
            let prev_count = self.active_count.fetch_add(1, Ordering::SeqCst);
            if prev_count == 0 {
                self.state.send_replace(State::Active);
            }
            KeepaliveGuard {
                active_count: self.active_count.clone(),
                state: self.state.clone(),
            }
        }
    }

    pub struct KeepaliveGuard {
        active_count: Arc<AtomicUsize>,
        state: watch::Sender<State>,
    }

    impl Drop for KeepaliveGuard {
        fn drop(&mut self) {
            let prev_count = self.active_count.fetch_sub(1, Ordering::SeqCst);
            if prev_count == 1 {
                self.state.send_replace(State::Idle);
            }
        }
    }

    pub struct Waiter {
        timeout: Duration,
        state: watch::Receiver<State>,
    }

    impl Waiter {
        /// # Cancel Safety
        ///
        /// This method is cancel safe.
        pub async fn wait(&mut self) -> Result<()> {
            loop {
                let state = *self.state.borrow_and_update();
                let changed = self.state.changed().map_err(|_| Error::Disconnected);

                match state {
                    State::Active => changed.await?,
                    State::Idle => tokio::select! {
                        _ = time::sleep(self.timeout) => break Ok(()),
                        r = changed => r?,
                    },
                }
            }
        }
    }

    pub fn new_pair(timeout: Duration) -> (Witness, Waiter) {
        let count = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = watch::channel(State::Idle);

        (
            Witness {
                active_count: count,
                state: tx,
            },
            Waiter { timeout, state: rx },
        )
    }
}

trait LogExt<T> {
    fn log_as_warning(self) -> Option<T>;
}

impl<T, E> LogExt<T> for std::result::Result<T, E>
where
    Error: From<E>,
    E: Debug,
{
    fn log_as_warning(self) -> Option<T> {
        if let Err(err) = &self {
            warn!("{err:?}");
        }
        self.ok()
    }
}

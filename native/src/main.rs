#![windows_subsystem = "windows"]

use std::{
    fmt::Debug,
    future::Future,
    io::ErrorKind,
    mem::{self, MaybeUninit},
    os::windows::io::AsRawHandle,
    panic::Location,
    pin::{pin, Pin},
    sync::{Arc, Barrier},
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use futures::{channel::oneshot, FutureExt};

use once_cell::sync::{Lazy, OnceCell};
use regex::Regex;
use single_instance::SingleInstance;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};
use tokio_util::sync::CancellationToken;
use tracing::Level;
#[allow(unused_imports)]
use tracing::{debug, error, info, trace, warn};
use windows::Win32::{
    Foundation::{BOOL, HANDLE, HWND, LPARAM, WPARAM},
    System::Threading::GetThreadId,
    UI::{
        Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_CONTROL, VK_OEM_3},
        WindowsAndMessaging::{
            DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowTextW, PeekMessageW,
            PostMessageA, PostQuitMessage, PostThreadMessageW, TranslateMessage, MSG, PM_NOREMOVE,
            WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
        },
    },
};

const WM_SHUTDOWN: u32 = WM_APP + 2;
const ID_HOTKEY_CTRLOEM3: usize = 2333;

const ID_INSTANCE: &str = "vscode_extension-ctrl_oem3-instance";
const ID_PIPE_SERVER: &str = env!("ctrl_oem3__named_pipe");

const ID_PROTO_GET_STATUS: u8 = 71;
const ID_PROTO_NOTIFY_STOP: u8 = 72;

const ID_PROTO_SAY_OK: u8 = 171;
const ID_PROTO_GRIPE_REGEX: u8 = 172;

static PATTERN_MATCHES_TITLE_DEFAULT: Lazy<Regex> =
    Lazy::new(|| Regex::new(env!("ctrl_oem3__matches_window_title")).unwrap());
static PATTERN_MATCHES_TITLE: OnceCell<Regex> = OnceCell::new();

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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(Level::INFO)
        .init();

    let cli = Cli::try_parse()?;
    PATTERN_MATCHES_TITLE
        .get_or_try_init(|| Regex::new(&cli.matches_window_title))
        .log_as_error();

    let ctrloem3 = SingleInstance::new(ID_INSTANCE)?;
    if !ctrloem3.is_single() {
        info!("An existing CtrlOEM3 service is reused. ");
        return Ok(());
    } else {
        info!("A new CtrlOEM3 service is created. ")
    }

    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let mut fut_forward_hotkey = pin!(forward_hotkey());
            let mut fut_keepalive_server = pin!(keepalive_server());

            tokio::select! {
                res = &mut fut_forward_hotkey => res,
                res = &mut fut_keepalive_server =>res,
            }
        });

    if let Err(ref err) = result {
        error!("{err:?}");
    }

    info!("CtrlOEM3 service stopped. ");

    result
}

fn forward_hotkey() -> ForwardHotkey {
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

        let res = forward_hotkey_sync();
        if let Err(err) = tx.send(res) {
            error!("Unable to send hotkey forwarding result: {err:?}");
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
            warn!("Failed to get thread id, handle={handle:x} tid={tid:x}. ");
        } else {
            unsafe {
                PostThreadMessageW(tid, WM_SHUTDOWN, None, None)
                    .ctx()
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
        self.notify_quit().log_as_error();
        unsafe {
            self.jh
                .assume_init_read()
                .join()
                .map_err(|_| anyhow!("Failed to join ForwardHotkey. "))
                .log_as_error();
        }
    }
}

fn forward_hotkey_sync() -> Result<()> {
    unsafe {
        RegisterHotKey(
            None,
            ID_HOTKEY_CTRLOEM3 as i32,
            MOD_CONTROL,
            VK_OEM_3.0 as _,
        )
    }
    .ctx()?;

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
            WM_HOTKEY if matches!(msg.wParam, WPARAM(ID_HOTKEY_CTRLOEM3)) => {
                try_mimic_ctrl_oem3();
            }
            WM_SHUTDOWN => unsafe {
                PostQuitMessage(0);
            },
            _ => {}
        }
    }

    unsafe {
        UnregisterHotKey(None, ID_HOTKEY_CTRLOEM3 as i32).ctx()?;
    }
    Ok(())
}

fn try_mimic_ctrl_oem3() {
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
        if !PATTERN_MATCHES_TITLE
            .get()
            .unwrap_or(&PATTERN_MATCHES_TITLE_DEFAULT)
            .is_match(&window_title)
        {
            return;
        }

        for action in [WM_KEYDOWN, WM_KEYUP] {
            PostMessageA(hwnd, action, WPARAM(VK_OEM_3.0 as usize), LPARAM(1))
                .ctx()
                .log_as_warning();
        }
    }
}

async fn keepalive_server() -> Result<()> {
    let (mut obs, mut idle) = idle::new_pair(Duration::from_secs(6));
    let mut idle = pin!(idle.wait());
    let token = CancellationToken::new();
    let mut cancelled = pin!(token.cancelled());

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(ID_PIPE_SERVER)?;

    loop {
        tokio::select! {
            res = server.connect() => res?,
            _ = &mut idle => break,
            _ = &mut cancelled => break,
        };

        let conn = server;
        server = ServerOptions::new().create(ID_PIPE_SERVER)?;

        tokio::spawn(handle_connection(conn, obs.get_active(), token.clone()));
    }

    Ok(())
}

async fn handle_connection(
    mut conn: NamedPipeServer,
    guard: idle::ActiveGuard,
    token: CancellationToken,
) {
    async move {
        let _guard = guard;
        let mut cancelled = pin!(token.cancelled());

        loop {
            let mut command = [0u8];

            tokio::select! {
                res = conn.read_exact(&mut command) => match res {
                    Err(err) if err.kind() == ErrorKind::UnexpectedEof => break,
                    res => {
                        res.ctx()?;
                    },
                },
                _ = &mut cancelled => break,
            }

            match command {
                [ID_PROTO_GET_STATUS] => {
                    let state = if PATTERN_MATCHES_TITLE.get().is_none() {
                        ID_PROTO_GRIPE_REGEX
                    } else {
                        ID_PROTO_SAY_OK
                    };
                    conn.write(&[state]).await?;
                    conn.flush().await?;
                }
                [ID_PROTO_NOTIFY_STOP] => {
                    token.cancel();
                }
                _ => bail!("Received unknown command: {command:?}"),
            }
        }

        Result::Ok(())
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
        #[error("the paired 'Observed' is dropped. ")]
        Disconnected,
    }

    type Result<T> = result::Result<T, Error>;

    pub struct Observed {
        active_count: Arc<AtomicUsize>,
        state: watch::Sender<State>,
    }

    impl Observed {
        pub fn get_active(&mut self) -> ActiveGuard {
            let prev_count = self.active_count.fetch_add(1, Ordering::SeqCst);
            if prev_count == 0 {
                self.state.send_replace(State::Active);
            }
            ActiveGuard {
                active_count: self.active_count.clone(),
                state: self.state.clone(),
            }
        }
    }

    pub struct ActiveGuard {
        active_count: Arc<AtomicUsize>,
        state: watch::Sender<State>,
    }

    impl Drop for ActiveGuard {
        fn drop(&mut self) {
            let prev_count = self.active_count.fetch_sub(1, Ordering::SeqCst);
            if prev_count == 1 {
                self.state.send_replace(State::Idle);
            }
        }
    }

    pub struct Idle {
        timeout: Duration,
        state: watch::Receiver<State>,
    }

    impl Idle {
        /// Cancel Safety
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

    pub fn new_pair(timeout: Duration) -> (Observed, Idle) {
        let count = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = watch::channel(State::Idle);

        (
            Observed {
                active_count: count,
                state: tx,
            },
            Idle { timeout, state: rx },
        )
    }
}

trait LogExt<T> {
    fn ctx(self) -> Result<T>;
    fn log_as_error(self) -> Option<T>;
    fn log_as_warning(self) -> Option<T>;
}

impl<T, E> LogExt<T> for std::result::Result<T, E>
where
    anyhow::Error: From<E>,
    E: Debug,
{
    #[track_caller]
    fn ctx(self) -> Result<T> {
        let loc = Location::caller();
        self.map_err(anyhow::Error::from)
            .with_context(|| format!("at {}:{}", loc.file(), loc.line()))
    }

    fn log_as_error(self) -> Option<T> {
        if let Err(err) = &self {
            error!("{err:?}");
        }
        self.ok()
    }

    fn log_as_warning(self) -> Option<T> {
        if let Err(err) = &self {
            warn!("{err:?}");
        }
        self.ok()
    }
}

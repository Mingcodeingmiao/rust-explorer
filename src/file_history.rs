//! In-memory inverse transactions based on successful Shell operation receipts.
//! No target is inferred after an operation and no existing destination is overwritten.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use windows::{
    Win32::{System::Com::CoTaskMemFree, UI::Shell::*},
    core::{HRESULT, PCWSTR, Ref, implement},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint(Vec<(PathBuf, u64, u64, u64, u32, u64, [u8; 16])>);
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    Move {
        from: PathBuf,
        to: PathBuf,
        fingerprint: Fingerprint,
        #[serde(default)]
        shell_name: Option<String>,
    },
    Recycle {
        path: PathBuf,
        fingerprint: Fingerprint,
    },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Transaction {
    pub actions: Vec<Action>,
}
impl Transaction {
    /// Exact UTF-8 JSON size of Request::History without cloning the transaction
    /// or allocating its serialized form. Reject unsendable history at insertion.
    pub fn request_bytes(&self) -> Result<usize> {
        #[derive(Serialize)]
        struct Body<'a> {
            transaction: &'a Transaction,
        }
        #[derive(Serialize)]
        struct Envelope<'a> {
            #[serde(rename = "History")]
            history: Body<'a>,
        }
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        serde_json::to_writer(
            &mut counter,
            &Envelope {
                history: Body { transaction: self },
            },
        )?;
        Ok(counter.0)
    }
    /// Includes allocated vector/path/string capacity, rather than just action
    /// count. UI history stacks can evict old transactions before retaining them.
    pub fn approximate_bytes(&self) -> usize {
        fn tree_bytes(tree: &Fingerprint) -> usize {
            tree.0.iter().fold(
                tree.0.capacity().saturating_mul(std::mem::size_of::<(
                    PathBuf,
                    u64,
                    u64,
                    u64,
                    u32,
                    u64,
                    [u8; 16],
                )>()),
                |total, entry| total.saturating_add(entry.0.capacity()),
            )
        }
        self.actions.iter().fold(
            std::mem::size_of::<Self>().saturating_add(
                self.actions
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Action>()),
            ),
            |total, action| {
                let allocated = match action {
                    Action::Move {
                        from,
                        to,
                        fingerprint,
                        shell_name,
                    } => from
                        .capacity()
                        .saturating_add(to.capacity())
                        .saturating_add(tree_bytes(fingerprint))
                        .saturating_add(shell_name.as_ref().map_or(0, String::capacity)),
                    Action::Recycle { path, fingerprint } => {
                        path.capacity().saturating_add(tree_bytes(fingerprint))
                    }
                };
                total.saturating_add(allocated)
            },
        )
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    pub inverse: Transaction,
    /// Present for undo/redo execution, containing only unexecuted source actions.
    pub remaining: Option<Transaction>,
    pub warning: Option<String>,
    pub error: Option<String>,
}
pub fn decode(output: &str) -> Option<Response> {
    output.lines().find_map(|line| {
        line.strip_prefix("history:")
            .and_then(|s| serde_json::from_str(s).ok())
    })
}
pub fn with_error(output: &str, error: String) -> Option<String> {
    let mut response = decode(output)?;
    response.error = Some(error);
    Some(format!(
        "history:{}",
        serde_json::to_string(&response).ok()?
    ))
}
fn emit(response: &Response) {
    if let Ok(json) = serde_json::to_string(response) {
        println!("history:{json}");
    }
}
fn fingerprint(path: &Path) -> Result<Fingerprint> {
    use std::os::windows::fs::MetadataExt;
    let started = std::time::Instant::now();
    let mut pending = vec![(path.to_path_buf(), PathBuf::new())];
    let mut records = Vec::new();
    while let Some((current, relative)) = pending.pop() {
        ensure!(
            records.len() < 50_000 && started.elapsed() < std::time::Duration::from_secs(3),
            "项目过大，无法安全建立撤销记录"
        );
        let meta = std::fs::symlink_metadata(&current)?;
        let (volume, identity) = file_identity(&current)?;
        // Restoring a child changes its parent's directory timestamp. Identity,
        // attributes and the complete child inventory protect directory contents;
        // including directory mtime would reject a later valid stacked undo.
        let ordinary_directory = meta.is_dir() && meta.file_attributes() & 0x400 == 0;
        records.push((
            relative.clone(),
            if ordinary_directory {
                0
            } else {
                meta.file_size()
            },
            if ordinary_directory {
                0
            } else {
                meta.last_write_time()
            },
            meta.creation_time(),
            meta.file_attributes(),
            volume,
            identity,
        ));
        // Never traverse reparse points into another tree or volume.
        if meta.is_dir() && meta.file_attributes() & 0x400 == 0 {
            for child in std::fs::read_dir(&current)? {
                let child = child?;
                pending.push((child.path(), relative.join(child.file_name())));
            }
        }
    }
    records.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Fingerprint(records))
}
fn file_identity(path: &Path) -> Result<(u64, [u8; 16])> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::*};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let mut info = FILE_ID_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )?;
    }
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}
fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn key(path: &Path) -> String {
    path.to_string_lossy().trim_end_matches('\\').to_lowercase()
}
fn vacant(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path),Err(error) if error.kind()==std::io::ErrorKind::NotFound)
}
fn recycled_item(path: &Path) -> bool {
    let parts = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>();
    parts.len() == 3
        && parts[0].eq_ignore_ascii_case("$Recycle.Bin")
        && parts[1].starts_with("S-1-")
        && parts[2].starts_with("$R")
}
fn shell_path(item: Ref<'_, IShellItem>) -> Option<PathBuf> {
    let item = item.as_ref()?;
    shell_item_path(item)
}
fn shell_item_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let text = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let value = text.to_string().ok();
        CoTaskMemFree(Some(text.0.cast()));
        value.map(PathBuf::from)
    }
}
fn recycled_shell_item(path: &Path) -> Result<IShellItem> {
    unsafe {
        let root: IShellItem =
            SHCreateItemFromParsingName(windows::core::w!("shell:RecycleBinFolder"), None)?;
        let enumeration: IEnumShellItems = root.BindToHandler(None, &BHID_EnumItems)?;
        let start = std::time::Instant::now();
        for _ in 0..50_000 {
            ensure!(
                start.elapsed() < std::time::Duration::from_secs(3),
                "回收站枚举超时，未执行恢复"
            );
            let mut fetched = 0;
            let mut batch = [None];
            enumeration.Next(&mut batch, Some(&mut fetched))?;
            if fetched == 0 {
                break;
            }
            if let Some(item) = batch[0].take() {
                if shell_item_path(&item).is_some_and(|actual| key(&actual) == key(path)) {
                    return Ok(item);
                }
            }
        }
    }
    anyhow::bail!("系统回收站中找不到原项目，未执行恢复")
}
fn restore_recycled(from: &Path, to: &Path, expected: &Fingerprint) -> Result<()> {
    use windows::{Win32::UI::WindowsAndMessaging::*, core::PCSTR};
    ensure!(vacant(to), "恢复位置已被占用，拒绝覆盖");
    // Shell extensions on some Windows builds restore the data but leave this
    // transaction's orphaned $I index. Capture that exact index identity first;
    // remove it only after a verified restore and only if it stayed unchanged.
    let info = from.with_file_name(format!(
        "$I{}",
        from.file_name()
            .context("缺少回收站名称")?
            .to_string_lossy()
            .trim_start_matches("$R")
    ));
    let info_before = fingerprint(&info).ok();
    let item = recycled_shell_item(from)?;
    unsafe {
        let menu: IContextMenu = item.BindToHandler(None, &BHID_SFUIObject)?;
        let popup = CreatePopupMenu()?;
        let queried = menu.QueryContextMenu(popup, 0, 1, 0x7fff, CMF_NORMAL);
        if queried.is_err() {
            let _ = DestroyMenu(popup);
            queried.ok()?;
        }
        let invocation = CMINVOKECOMMANDINFO {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
            lpVerb: PCSTR(b"undelete\0".as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let invoked = menu.InvokeCommand(&invocation);
        let _ = DestroyMenu(popup);
        invoked?;
    }
    let start = std::time::Instant::now();
    while from.exists() || !to.exists() {
        ensure!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "系统恢复尚未完成，已停止后续操作"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    ensure!(
        &fingerprint(to)? == expected,
        "系统恢复后项目身份不匹配，已停止后续操作"
    );
    if let Some(before) = info_before {
        if fingerprint(&info).is_ok_and(|after| after == before) {
            std::fs::remove_file(&info).context("已恢复文件，但无法清理本次回收站索引")?;
        }
    }
    Ok(())
}
#[derive(Default)]
struct State {
    roots: BTreeSet<String>,
    before: BTreeSet<String>,
    response: Response,
    enabled: bool,
    strict: bool,
    expected: BTreeMap<String, Fingerprint>,
    transaction: Option<Transaction>,
    completed: BTreeSet<String>,
    pending: Vec<(PathBuf, PathBuf, bool, Option<String>)>,
}
#[implement(IFileOperationProgressSink)]
struct Sink {
    state: Arc<Mutex<State>>,
}
impl Sink_Impl {
    fn remember_destinations(state: &mut State, folder: &Path) {
        let started = std::time::Instant::now();
        let scan = (|| -> std::io::Result<()> {
            for (index, item) in std::fs::read_dir(folder)?.enumerate() {
                if index >= 50_000 || started.elapsed() > std::time::Duration::from_millis(500) {
                    return Err(std::io::Error::other("destination enumeration budget"));
                }
                state.before.insert(key(&item?.path()));
            }
            Ok(())
        })();
        if scan.is_err() {
            state.response.warning = Some("目标目录无法完整查询；本次操作不可撤销".into());
        }
    }
    fn recheck(&self, source: &IShellItem) -> windows::core::Result<()> {
        let state = self.state.lock().unwrap();
        if !state.strict {
            return Ok(());
        }
        let path = shell_item_path(source).ok_or_else(|| {
            windows::core::Error::from_hresult(windows::Win32::Foundation::E_ABORT)
        })?;
        if let Some(expected) = state.expected.get(&key(&path)) {
            if !fingerprint(&path).is_ok_and(|actual| &actual == expected) {
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_ABORT,
                ));
            }
        }
        Ok(())
    }
    fn pre(&self, source: Ref<'_, IShellItem>, destination: Ref<'_, IShellItem>) {
        let Some(source) = shell_path(source) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if !state.roots.contains(&key(&source)) {
            return;
        }
        if let Some(folder) = shell_path(destination) {
            Self::remember_destinations(&mut state, &folder);
        }
    }
    fn post(
        &self,
        source: Ref<'_, IShellItem>,
        target: Ref<'_, IShellItem>,
        hr: HRESULT,
        copied: bool,
    ) {
        let parsing_name = target.as_ref().and_then(|item| unsafe {
            let text = item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING).ok()?;
            let value = text.to_string().ok();
            CoTaskMemFree(Some(text.0.cast()));
            value
        });
        let Some(source) = shell_path(source) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if !state.roots.contains(&key(&source)) {
            return;
        }
        if hr.is_err() {
            // Failed recursive operations can still leave some copied children.
            // No guessed inverse is safe; preserve success receipts only when
            // no partial failed root was reported.
            if shell_path(target).is_some() {
                state.response.warning =
                    Some("部分项目操作失败，系统可能已创建部分内容；本批次不可撤销".into());
            }
            return;
        }
        state.completed.insert(key(&source));
        let Some(target) = shell_path(target) else {
            state.response.warning = Some("系统未返回实际目标，无法安全撤销本次操作".into());
            return;
        };
        if source == target {
            return;
        }
        if state.before.contains(&key(&target)) {
            state.response.warning =
                Some("本次包含覆盖或目录合并，不提供撤销，避免损坏原有文件".into());
            return;
        }
        // Shell reports directory creation before recursively copying children.
        // Capture the final tree only after all operations have finished.
        state.pending.push((source, target, copied, parsing_name));
    }
}
#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(
        &self,
        _: u32,
        s: Ref<'_, IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        // The source parent is the rename destination directory.
        if let Some(path) = shell_path(s) {
            if let Some(parent) = path.parent() {
                let mut state = self.state.lock().unwrap();
                Self::remember_destinations(&mut state, parent);
            }
        }
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        s: Ref<'_, IShellItem>,
        _: &PCWSTR,
        h: HRESULT,
        t: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.post(s, t, h, false);
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        s: Ref<'_, IShellItem>,
        d: Ref<'_, IShellItem>,
        name: &PCWSTR,
    ) -> windows::core::Result<()> {
        if let Some(source) = s.as_ref() {
            self.recheck(source)?;
        }
        if self.state.lock().unwrap().strict {
            let target = d
                .as_ref()
                .and_then(shell_item_path)
                .and_then(|folder| unsafe { name.to_string().ok() }.map(|name| folder.join(name)));
            if target.as_ref().is_none_or(|path| !vacant(path)) {
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_ABORT,
                ));
            }
        }
        self.pre(s, d);
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        s: Ref<'_, IShellItem>,
        _: Ref<'_, IShellItem>,
        _: &PCWSTR,
        h: HRESULT,
        t: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.post(s, t, h, false);
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        s: Ref<'_, IShellItem>,
        d: Ref<'_, IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.pre(s, d);
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        s: Ref<'_, IShellItem>,
        _: Ref<'_, IShellItem>,
        _: &PCWSTR,
        h: HRESULT,
        t: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.post(s, t, h, true);
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, s: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        if let Some(source) = s.as_ref() {
            self.recheck(source)?;
        }
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        _: u32,
        s: Ref<'_, IShellItem>,
        h: HRESULT,
        t: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.post(s, t, h, false);
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<'_, IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<'_, IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}
/// Prints successful inverse receipts even when PerformOperations later fails.
/// Its lifetime must cover all queued operations and GetAnyOperationsAborted.
pub struct Recorder {
    op: IFileOperation,
    cookie: u32,
    state: Arc<Mutex<State>>,
}
impl Recorder {
    pub fn attach(op: &IFileOperation, paths: &[PathBuf]) -> Result<Self> {
        let state = Arc::new(Mutex::new(State {
            roots: paths.iter().map(|p| key(p)).collect(),
            enabled: true,
            ..State::default()
        }));
        let sink: IFileOperationProgressSink = Sink {
            state: state.clone(),
        }
        .into();
        let cookie = unsafe { op.Advise(&sink)? };
        Ok(Self {
            op: op.clone(),
            cookie,
            state,
        })
    }
    pub fn created(&self, path: &Path) {
        let mut state = self.state.lock().unwrap();
        match fingerprint(path) {
            Ok(fingerprint) => state.response.inverse.actions.push(Action::Recycle {
                path: path.into(),
                fingerprint,
            }),
            Err(error) => state.response.warning = Some(format!("无法建立安全撤销记录：{error}")),
        }
    }
}
impl Drop for Recorder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.op.Unadvise(self.cookie);
        }
        let mut state = self.state.lock().unwrap();
        if !state.enabled {
            return;
        }
        for (source, target, copied, shell_name) in std::mem::take(&mut state.pending) {
            match fingerprint(&target) {
                // Restore redo must recycle afresh; never manufacture a stale $R/$I pair.
                Ok(fingerprint) => {
                    state
                        .response
                        .inverse
                        .actions
                        .push(if copied || recycled_item(&source) {
                            Action::Recycle {
                                path: target,
                                fingerprint,
                            }
                        } else {
                            Action::Move {
                                from: target,
                                to: source,
                                fingerprint,
                                shell_name,
                            }
                        })
                }
                Err(error) => {
                    state.response.warning = Some(format!("无法建立安全撤销记录：{error}"))
                }
            }
        }
        // A mixed operation containing a destructive merge is a history barrier.
        if state.response.warning.is_some() {
            state.response.inverse.actions.clear();
        }
        state.response.remaining = state.transaction.as_ref().map(|transaction| Transaction {
            actions: transaction
                .actions
                .iter()
                .filter(|action| {
                    let source = match action {
                        Action::Move { from, .. } => from,
                        Action::Recycle { path, .. } => path,
                    };
                    !state.completed.contains(&key(source))
                })
                .cloned()
                .collect(),
        });
        state.response.inverse.actions.reverse();
        emit(&state.response);
    }
}
pub fn record_created(path: &Path) -> Result<()> {
    let response = Response {
        inverse: Transaction {
            actions: vec![Action::Recycle {
                path: path.to_path_buf(),
                fingerprint: fingerprint(path)?,
            }],
        },
        ..Response::default()
    };
    emit(&response);
    Ok(())
}
pub fn barrier(reason: &str) {
    emit(&Response {
        warning: Some(reason.into()),
        ..Response::default()
    });
}
/// Runs on the disposable Shell helper STA. Preflight all sources and collisions
/// before queuing any mutations; every move uses the actual successful target.
pub fn apply(transaction: Transaction) -> Result<()> {
    use windows::Win32::System::Com::*;
    ensure!(!transaction.actions.is_empty(), "没有可撤销的操作");
    let mut sources = BTreeSet::new();
    for action in &transaction.actions {
        let (source, expected) = match action {
            Action::Move {
                from, fingerprint, ..
            } => (from, fingerprint),
            Action::Recycle { path, fingerprint } => (path, fingerprint),
        };
        ensure!(
            source.is_absolute() && source.parent().is_some(),
            "拒绝处理磁盘根目录"
        );
        ensure!(sources.insert(key(source)), "撤销记录含重复项目");
        ensure!(
            &fingerprint(source).context("原项目不存在，无法撤销")? == expected,
            "项目已被修改或加入其他文件，拒绝撤销：{}",
            source.display()
        );
        if let Action::Move { to, .. } = action {
            ensure!(
                vacant(to),
                "原位置已有项目或无法读取，拒绝覆盖：{}",
                to.display()
            );
            ensure!(to.parent().is_some_and(Path::is_dir), "原目标目录不存在");
        }
    }
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
        let _cancel = crate::shell::attach_cancellation(&op)?;
        op.SetOperationFlags(FILEOPERATION_FLAGS(
            FOFX_RECYCLEONDELETE.0 | FOF_WANTNUKEWARNING.0 | FOF_RENAMEONCOLLISION.0,
        ))?;
        let roots = transaction
            .actions
            .iter()
            .map(|a| match a {
                Action::Move { from, .. } => from.clone(),
                Action::Recycle { path, .. } => path.clone(),
            })
            .collect::<Vec<_>>();
        let _recorder = Recorder::attach(&op, &roots)?;
        {
            let mut state = _recorder.state.lock().unwrap();
            state.strict = true;
            state.transaction = Some(transaction.clone());
            state.expected = transaction
                .actions
                .iter()
                .map(|a| match a {
                    Action::Move {
                        from, fingerprint, ..
                    } => (key(from), fingerprint.clone()),
                    Action::Recycle { path, fingerprint } => (key(path), fingerprint.clone()),
                })
                .collect();
        }
        for action in transaction.actions {
            match action {
                Action::Move {
                    from,
                    to,
                    shell_name,
                    fingerprint,
                } => {
                    if recycled_item(&from) {
                        let restored = restore_recycled(&from, &to, &fingerprint);
                        if restored.is_err()
                            && !from.exists()
                            && self::fingerprint(&to).is_ok_and(|actual| actual == fingerprint)
                        {
                            let mut state = _recorder.state.lock().unwrap();
                            state.completed.insert(key(&from));
                            state.response.warning =
                                Some("文件已恢复，但系统恢复清理未完成；已停止后续操作".into());
                        }
                        restored?;
                        let mut state = _recorder.state.lock().unwrap();
                        state.completed.insert(key(&from));
                        state.pending.push((from, to, false, None));
                        continue;
                    }
                    let source_name = shell_name
                        .map(PathBuf::from)
                        .unwrap_or_else(|| from.clone());
                    let source: IShellItem =
                        SHCreateItemFromParsingName(PCWSTR(wide(&source_name).as_ptr()), None)?;
                    let destination: IShellItem = SHCreateItemFromParsingName(
                        PCWSTR(wide(to.parent().unwrap()).as_ptr()),
                        None,
                    )?;
                    let name = wide(Path::new(to.file_name().context("缺少目标名称")?));
                    op.MoveItem(&source, &destination, PCWSTR(name.as_ptr()), None)?;
                }
                Action::Recycle { path, .. } => {
                    let source: IShellItem =
                        SHCreateItemFromParsingName(PCWSTR(wide(&path).as_ptr()), None)?;
                    op.DeleteItem(&source, None)?;
                }
            }
        }
        // A batch consisting only of recycle restores has no queued IFileOperation.
        if _recorder.state.lock().unwrap().completed.len() == roots.len() {
            return Ok(());
        }
        op.PerformOperations()?;
        ensure!(!op.GetAnyOperationsAborted()?.as_bool(), "撤销或重做已取消");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_budget_accounts_for_directory_records_not_just_action_count() {
        let large = Transaction {
            actions: vec![Action::Recycle {
                path: PathBuf::from("C:\\owned-fixture"),
                fingerprint: Fingerprint(
                    (0..1000)
                        .map(|index| {
                            (
                                PathBuf::from(format!("{index}-{}", "x".repeat(1000))),
                                0,
                                0,
                                0,
                                0,
                                0,
                                [0; 16],
                            )
                        })
                        .collect(),
                ),
            }],
        };
        assert!(large.approximate_bytes() > 1_000_000);
        assert_eq!(
            large.request_bytes().unwrap(),
            serde_json::to_vec(&crate::shell::Request::History {
                transaction: large.clone(),
            })
            .unwrap()
            .len()
        );
        assert!(Transaction::default().approximate_bytes() < 1024);
    }
    #[test]
    fn tree_fingerprint_rejects_external_child_but_allows_undone_child_creation() {
        let folder = std::env::temp_dir().join(format!(
            "rust-explorer-history-unit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&folder).unwrap();
        let initial = fingerprint(&folder).unwrap();
        let child = folder.join("external.txt");
        std::fs::write(&child, b"keep").unwrap();
        assert_ne!(fingerprint(&folder).unwrap(), initial);
        std::fs::remove_file(&child).unwrap();
        assert_eq!(fingerprint(&folder).unwrap(), initial);
        std::fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn partial_failure_receipt_keeps_inverse_and_remaining() {
        let output = "history:{\"inverse\":{\"actions\":[]},\"remaining\":{\"actions\":[]},\"warning\":null,\"error\":null}";
        let result = with_error(output, "failed after receipt".into()).unwrap();
        let response = decode(&result).unwrap();
        assert_eq!(response.error.as_deref(), Some("failed after receipt"));
        assert!(response.remaining.is_some());
        assert!(decode("not a history response").is_none());
    }
    #[test]
    fn only_actual_recycle_root_items_use_native_restore() {
        assert!(recycled_item(Path::new(
            "C:\\$Recycle.Bin\\S-1-5-21-1\\$Rabc.txt"
        )));
        assert!(!recycled_item(Path::new(
            "C:\\work\\$Recycle.Bin\\S-1-5-21-1\\$Rabc.txt"
        )));
        assert!(!recycled_item(Path::new(
            "C:\\$Recycle.Bin\\S-1-5-21-1\\$Iabc.txt"
        )));
    }
}

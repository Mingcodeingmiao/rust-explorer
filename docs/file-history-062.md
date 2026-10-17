# 文件操作撤销与重做（0.6.2）

本模块只记录本程序发起、成功返回真实目标的操作。第三方或系统菜单内部独立执行的文件操作没有冒充本程序历史。历史保存在窗口内存，进程退出后不继续撤销。

实现：src/file_history.rs 使用 IFileOperationProgressSink 的成功回执，取得实际目标，不猜复制后名称，也不猜回收站随机名称。目录复制的最终快照在 PerformOperations 完成后建立，避免 Shell 在子文件复制前触发根目录回执导致空目录快照。每个文件记录卷 ID、文件 ID、大小、时间和属性；目录还记录完整相对路径清单。不会沿 junction/symlink 遍历其他目录。

复制、新建的撤销把新增项目交给系统回收站；移动、重命名的撤销移动回真实原位置；删除的撤销调用系统回收站 namespace 的 undelete。恢复后核验文件身份，并且只清理由本次回执定位、身份未变的残留回收站索引。重做删除使用一次新的系统回收站操作，不制造旧 $R/$I 配对。恢复后生成另一方向的真实回执。

每次撤销或重做会先验证整批源项目及目标空缺。外部修改、添加子文件、文件替换、原位置被占用、权限查询失败都会拒绝执行。目录时间戳不参与比较，因为撤销后续子文件操作本身会改变目录时间；目录身份、属性、子项目清单及每个子文件身份仍参与比较。操作开始前回调再次检查身份和目标冲突。

父 UI API：

- shell::Request::History { transaction } 执行事务。
- shell::Request::RecordCreated { path } 只为已成功创建的本程序文件/目录建立记录。
- file_history::decode(output) 解析 Response，包含 inverse、remaining、warning、error。
- 普通成功操作 inverse 进入撤销栈并清空重做栈。
- 撤销/重做返回 inverse 进入另一栈，remaining 替换源栈记录；成功的 remaining 为空。
- 部分失败保留已成功回执和尚未执行的 remaining；execute_output 将辅助进程错误写入 error，避免丢掉标准输出中的成功操作。
- warning 表示历史屏障，两栈清空并向用户说明；永久删除同样是屏障。

边界：覆盖或目录合并无法仅凭回执恢复被覆盖的旧内容，本版明确设置历史屏障。没有伪造“覆盖可撤销”。快照最多 50,000 项/3秒，目标目录枚举最多 50,000 项/500ms，超出后操作可以成功但会提示无法安全记录历史。回收站枚举最多 50,000 项/3秒；不存在或不能确认的项目不会按物理路径盲目恢复。文件/目录若无法读取稳定身份也不能进入可安全撤销历史。无损正常事务与这些明确不可撤销场景需区分。

自有 Temp fixtures 验证：.tools/verify_history_062.py；日志 .tools/verify-history-062.log。覆盖文件复制、目录复制三轮撤销/重做、目录移动、重命名、回收站删除恢复、新建空目录、撤销新增子文件后再撤销更早父目录复制；拒绝外来子文件、已修改文件及目标占用；整批第二项无效时第一项也不会被修改；恢复后无对应残留 $I 元数据。没有对真实用户文件执行测试操作，没有操作全盘格式化或权限设置。

2026-10-17：无默认特性的全部库测试 47 项通过，包含本模块的目录指纹、回收站路径分类和部分失败响应测试。GUI 快捷键/菜单接入及发布包验收由父任务继续完成。

## 大事务传输与内存预算

系统操作现在统一使用 --shell-stdin 协议，stdin 接收最大 32MiB JSON，保留旧 --shell 参数作为兼容入口。父进程并发写 stdin、读 stdout、读 stderr，避免大指纹数据和大回执互相填满管道。stdout 上限96MiB、stderr上限1MiB；超限仍排空管道后报告错误，不会等待一个永远写不完的辅助进程。超过32MiB的请求在启动进程前就拒绝，不会发生部分写入后执行。

Transaction::approximate_bytes() 计算 Vec、PathBuf、String 的已分配 capacity 和指纹记录本体，用于父 UI 对 undo+redo 设置合计64MiB预算、逐出最旧事务。Transaction::request_bytes() 以计数Writer精确计算实际 History 请求JSON的UTF8字节数，不克隆事务、不创建整段JSON；超过 shell::MAX_REQUEST_BYTES 的单条历史不应保留成一个永远无法发送的撤销项。

新增真实fixture：.tools/verify_history_stdin_062.py，用1000个长文件名建立实际目录，History请求约261KiB，超过Windows命令行32KiB限制。通过 examples/history_stdin_probe.rs 直接调用实际 execute_output 父子协议完成RecordCreated→Undo→Redo，文件数、内容和回收站索引均断言。另使用 RUST_EXPLORER_OWNED_PIPE_STRESS 在子进程读取stdin前先写10MiB stdout与256KiB stderr，仍通过全部操作，验证三管道并发没有死锁。日志：.tools/verify-history-stdin-062.log、.tools/verify-history-pipe-stress-062.log。

快速访问 pin/unpin 作为独立系统扩展调用有5秒截止并终止辅助进程；Copy/Move/Delete及用户打开的原生菜单不套用这一超时。辅助进程启动时转交本应用前台权限，支持系统菜单键盘关闭。

最终构建检查通过，库测试50项通过，包含超长请求解析、超限输入拒绝、超限输出排空和目录历史内存核算测试。

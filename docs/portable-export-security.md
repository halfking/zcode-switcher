# Portable 导出安全性评估：密码加密与原子回滚

> 评估时间：2026-09-13 · 基线提交：`3dfc58c fix(auth): harden cross-platform credential migration`
>
> 结论先行：**建议两项都做**。密码加密优先级高（portable 文件等价于账号本体，落地即明文 token）；
> 原子写入属于低成本修复（导出侧立刻做），导入侧的"整批事务回滚"建议做成"全部校验通过后才提交"，
> 现有的"部分成功 + 报告"语义可以保留。

## 1. 现状

### 1.1 数据流

| 环节 | 实现 | 说明 |
| --- | --- | --- |
| 导出 | `profile.rs` `portable_account_from_profile` | 从本机档案库读取凭据副本，`decrypt_portable_credentials` 解密为**明文**后序列化进 JSON/ZIP |
| 传输 | 任意通道（共享文件夹、下载目录、聊天工具） | 文件内容即明文 OAuth token，任何人拿到即可导入使用 |
| 导入 | `import_portable_account` → `encrypt_portable_credentials` | 目标机重新用本机 fallback key 加密落盘 |

导出 schema 为 `zcode-switcher-account/v1`，文件名形如
`zcode-account-<name>-<id>-<时间戳>.json`（单账号）或 `zcode-accounts-<n>-<时间戳>.zip`（批量）。

### 1.2 已确认的风险点（按严重程度）

1. **导出文件无任何保护（高）**：`credentials` 字段在文件里是明文 JSON 字符串，
   含长期有效的 access/refresh token。文件经常落在 `Downloads`、共享目录（`\\Mac\Home`）、
   桌面等位置，回收站/云同步/聊天工具都会额外留副本。实际使用流程（跨虚机迁移）加剧暴露面。
2. **导出写入非原子（中）**：单账号导出用 `fs::write`，ZIP 导出用 `fs::File::create` 后顺序写
   （`profile.rs` `write_profiles_zip` / `export_profile_to_file`）。磁盘满、进程被杀、
   移动介质断开都会留下**截断文件**；更糟的是失败时目标路径可能残留**部分明文**。
3. **批量导入无整批事务（中低）**：`import_profiles_from_files` 逐个账号提交，
   第 N 个失败时前 N-1 个已落库。UI 有 imported/skipped/failed 报告，用户可感知；
   但"同一 ZIP 里一半成功一半失败"是否合意需要产品决定。
   真正的问题在**单账号导入的写盘顺序**：必须保证"先写凭据副本文件、后原子更新索引"，
   否则可能出现索引指向不存在快照的悬空引用（索引已是 `atomic_write`，快照写入需复核失败路径）。
4. **索引/快照删除路径（低）**：去重与删除时已避免删除仍被引用的快照（`dedup_before_save`），
   未发现明显丢数据路径。

## 2. 建议

### 2.1 密码加密导出（建议做，优先级高）

- **schema 升级 `zcode-switcher-account/v2`**：新增
  `encryption: { scheme: "age/aes-gcm", kdf, salt, nonce }`，`credentials` 与
  `provider_api_keys` 整体加密为一个 `ciphertext` 字段；`profile` 身份字段保留明文
  （供导入前去重展示，不含 secret）。
- **KDF**：优先 Argon2id（Rust 生态 `argon2` crate 成熟）；保守起见 PBKDF2-HMAC-SHA256
  ≥ 600k 次也可接受。加密本体沿用现有 `aes_gcm` 依赖（AES-256-GCM），格式复用
  `enc:v1:` 风格标记（如 `penc:v1:`）以便复用校验逻辑。
- **兼容性**：v1 明文包继续可导入；导出 UI 增加可选密码框——留空按 v1 导出（兼容旧版
  Switcher），填密码则输出 v2。**默认值建议为空 + 明显的安全提示**，避免用户误以为
  v1 文件是加密的。
- **导入端**：检测到 v2 时弹密码框，错误密码报"解密失败"，不落盘任何内容。
- **成本估计**：Rust 侧约 150-250 行 + 单测；前端一个密码输入弹窗。1-2 天工作量。

### 2.2 原子写入与回滚

- **导出（建议立刻做，成本极低）**：`write_profiles_zip` 与 `export_profile_to_file`
  改为写 `同目录/<name>.tmp` → `fs::rename` 覆盖目标。仓库里已有现成
  `atomic_write`（`profile.rs:619`），抽为公共函数复用即可。失败时清理 .tmp，不残留明文。
- **导入（建议做"预校验 + 有序提交"）**：
  1. 先把所有输入全部解析、解密、校验（schema、身份字段、enc:v1 拒绝）到内存；
  2. 全部通过后逐个提交，顺序固定为"写凭据副本 → 原子更新索引"；
  3. 任一账号在**校验阶段**失败则整批拒绝（一个都没写），**提交阶段**失败则停止并在报告中
     标明已完成数量（此时不做删除式回滚——回滚本身可能误删同名既有档案，风险大于收益）。
- **不推荐做删除式整批回滚**：导入可能更新既有档案（同身份去重），回滚意味着恢复旧内容，
  需要备份/恢复整套机制，复杂度与出错面都大；"预校验"已消除绝大多数中途失败。

## 3. 已补充的回归覆盖（2026-09-13）

- `profile.rs` tests：portable 凭据"明文→加密→解密"往返、`enc:v1:` 双重加密输入拒绝、
  非 object 输入拒绝（跨平台，Windows 实机可跑 `cargo test --lib profile::tests`）。
- `scripts/windows-regression.ps1`（`npm run test:windows`）：真实用户桌面会话下的实机检查——
  已安装 Switcher 版本、Switcher/ZCode 进程归属交互会话、`credentials.json` 与账号池快照
  全部为 `enc:v1:` 密文、ZCode CDP 9229 渲染页面存在。脚本只输出计数/路径/掩码邮箱，不输出任何 token。

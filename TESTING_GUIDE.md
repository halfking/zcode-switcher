# 测试指南

## 前置条件
- Rust 工具链已安装
- Node.js 环境
- ZCode 客户端运行中

## 快速测试

### 1. 构建应用
```bash
npm install
npm run tauri build
```

### 2. 运行真机自测

#### 测试网关拦截（pause模式）
```bash
# Windows
.\src-tauri\target\release\zcode-switcher.exe --guard-selftest

# macOS/Linux
./src-tauri/target/release/zcode-switcher --guard-selftest
```

**预期输出**：
```
quota_guard_set_for_test(true): ...
quota_guard_status: paused=true
  → 本次自测手动启用了额度守护，请通过前端 UI 关闭后继续使用。
```

#### 测试套餐切换
```bash
# Windows
.\src-tauri\target\release\zcode-switcher.exe --plan-switch-probe

# macOS/Linux
./src-tauri/target/release/zcode-switcher --plan-switch-probe
```

**预期输出**：
```
CDP ok, setting service reachable
Current selected keys: {...}
Plan switch completed successfully
```

### 3. GUI功能测试

#### 测试pause模式
1. 启动应用
2. 打开设置面板（Settings）
3. 找到"低额度时动作"（Action on low quota）
4. 选择"暂停执行"（Pause execution）
5. 设置低阈值（如Token万：1，积分：100）
6. 等待触发
7. **验证**：
   - 显示toast警告："额度守护已暂停"
   - ZCode请求被拦截（返回429错误）
   - 不自动切换账号

#### 测试套餐优先切换
1. 选择"自动切换套餐/账号"模式
2. 准备测试账号：
   - 账号A有两个套餐
   - Start Plan：余额充足
   - GLM Coding Plan：余额低（当前使用）
3. 设置阈值触发切换
4. **验证**：
   - 显示"正在切换到 Start Plan..."
   - 成功切换到同账号的Start Plan
   - **不切换账号**

#### 测试账号切换
1. 让账号A所有套餐都低于阈值
2. 账号B有充足余额
3. 触发切换
4. **验证**：
   - 先尝试账号A内切换（失败，无高余额套餐）
   - 然后切换到账号B
   - 显示"已切换到账号B"

#### 测试配置持久化
1. 选择"暂停执行"
2. 关闭应用
3. 重新打开应用
4. **验证**：设置保持为"暂停执行"

### 4. 单元测试
```bash
cd src-tauri
cargo test
```

## 常见问题

### Q: 套餐切换没有生效？
A: 检查：
1. ZCode是否运行中
2. CDP端口是否正确（默认9223）
3. 查看日志：`~/.zcode-switcher/logs/`

### Q: pause模式下请求没有被拦截？
A: 检查：
1. 网关是否启用（Settings -> Enable Gateway）
2. ZCode是否配置了正确的代理地址
3. 后端日志：查看quota_guard状态

### Q: 如何查看详细日志？
A: 
- Windows: `%APPDATA%\.zcode-switcher\logs\`
- macOS: `~/Library/Application Support/zcode-switcher/logs/`
- Linux: `~/.local/share/zcode-switcher/logs/`

## 回归测试清单

- [ ] pause模式：低余额时拦截请求
- [ ] pause模式：toast通知显示
- [ ] switch模式：套餐优先切换
- [ ] switch模式：所有套餐低时切账号
- [ ] switch模式：所有账号低时暂停
- [ ] 配置持久化：重启后保持
- [ ] CDP切换：实时生效不重启ZCode
- [ ] 配置文件回退：CDP不可用时写文件
- [ ] 冷却期：防止频繁切换
- [ ] 错误提示：切换失败有toast
- [ ] 国际化：中英俄文案正确

## 性能测试

### 切换延迟
```bash
# 记录切换开始到完成的时间
# 预期：CDP模式 < 2秒，配置文件模式 < 5秒
```

### 网关拦截延迟
```bash
# 发送请求到收到429响应的时间
# 预期：< 100ms
```

## 调试技巧

### 启用详细日志
```bash
# 设置环境变量
RUST_LOG=debug npm run tauri dev
```

### 查看CDP通信
打开ZCode DevTools (Ctrl+Shift+I)，查看Console输出

### 手动测试CDP
```javascript
// 在ZCode DevTools Console中执行
await window.zcode.settingService.update({
  modelProviderFamilySelectedKeys: {
    chatglm: "glm-coding-start"
  }
})
```

## 已知限制

1. **CDP连接**：需要ZCode运行中且CDP端口可访问
2. **配置文件回退**：仅在CDP不可用时生效，需要重启ZCode
3. **冷却期**：成功切换后5秒内不重试，失败后30秒内不重试
4. **网关模式**：需要ZCode配置代理到本地网关

## 参考文档

- 实现细节：[docs/fixes-2026-09-16-plan-switch.md](docs/fixes-2026-09-16-plan-switch.md)
- 更新日志：[docs/changelog.md](docs/changelog.md)
- 使用文档：[docs/usage.md](docs/usage.md)

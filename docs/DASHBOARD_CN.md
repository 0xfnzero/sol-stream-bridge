# React 管理面板

[English](DASHBOARD.md) · [共享订阅](SHARED_CN.md) · [项目首页](../README_CN.md)

本功能在 `main` 源码中，**不包含在已发布的 v0.1.1 安装包中**。面板运行在共享 Yellowstone 接收端 B，用于管理 Bot 用户和查看转发状态。前端使用 React + TypeScript，Rust 进程同时提供静态页面和管理 API，无需独立 Web 服务。

## 能管理什么

- 真实运行状态：上游连接、下游 TCP 连接、活动订阅、用户用量、合并过滤器/账户数、消息分发速率、拒绝请求、慢客户端和上游中断。
- 用户：创建随机 API Key、设置订阅/过滤器/账户累计额度、允许或禁止账户更新、停用、启用、轮换 Key、删除。
- 订阅：查看来源 IP、订阅时间、已入队分发消息数，断开指定 Subscribe 流。列表最多显示 200 条；停用用户可断开该用户全部订阅。
- 访问策略：IP 白名单和独立的“要求用户 API Key”开关。开启时必须同时通过 Key 和 IP 检查；关闭时按来源 IP 分配用户额度。

Shred 路由、默认透传模式、监听地址、TLS、供应商凭证及全局上游额度仍由 TOML 管理。面板管理的是共享 gRPC 接收端，并非所有转发方式的通用配置编辑器。计数与速率反映接收/入队消息，不是网络吞吐字节、Bot 已处理确认或端到端延迟测量；历史曲线仅保存在当前页面内存。

## 源码启动

先完成 [共享模式部署](SHARED_CN.md)，确保源 A 与接收端 B 的隧道配置一致。在 B 的仓库目录中执行：

```bash
# 首次构建页面需要 Node.js >=22.12、npm；生成令牌需要 openssl
./setup-dashboard.sh
```

脚本生成权限为 `600` 的 `.dashboard-token`，保留已有令牌，缺少静态页面时自动安装前端依赖并构建。将下表添加到实际使用的共享接收端配置中（不要重复添加）：

```toml
[grpc.shared.dashboard]
listen = "127.0.0.1:8080"
token_env = "DASHBOARD_TOKEN"
state_file = "dashboard-state.toml"
assets_dir = "dashboard/dist"
```

`config.shared-receiver.toml` 已提供注释模板。新安装也可用 `./setup.sh shared-receiver` 创建配置和隧道凭证；已有 `config.toml` 会保留，不会自动切换模式。编辑 `api_keys`、白名单、上游和隧道设置后检查并启动：

```bash
cargo build --release --locked
./run.sh config.shared-receiver.toml --check
./run.sh config.shared-receiver.toml
```

若使用 `config.toml` 则省略文件参数。`run.sh` 自动把 `.dashboard-token` 作为数据读入环境变量 `DASHBOARD_TOKEN`；已导出的同名变量优先。自定义 `token_env` 时自行导出对应变量。直接运行二进制时也需提供环境变量，并从项目/安装目录运行，因为配置内路径相对于工作目录。启用后访问 [http://127.0.0.1:8080](http://127.0.0.1:8080)，在自己终端读取 `.dashboard-token` 并输入登录框。

后续 Linux Release 打包流程会包含构建好的 `dashboard/dist` 和初始化脚本，届时安装包运行无需 Node.js。当前不要用旧 v0.1.1 二进制尝试启用此配置。

## 远程管理

面板强制监听 loopback，不开放到公网或局域网。通过 SSH 转发访问：

```bash
ssh -N -L 8080:127.0.0.1:8080 user@RECEIVER_IP
```

在本地浏览器打开 `http://127.0.0.1:8080`。若本地端口已占用，可改用 `-L 18080:127.0.0.1:8080` 和 `http://127.0.0.1:18080`。管理 HTTP 走 SSH 通道，不改变 Bot、上游或桥接隧道的传输设置。Host 仅接受 localhost/127.0.0.1/::1，跨来源浏览器请求会被拒绝。

管理员令牌与用户 API Key、`BRIDGE_TOKEN`、供应商 `x-token` **互相独立**。管理员令牌只在浏览器内存保存，刷新/退出需重新登录，不写入 localStorage、Cookie 或 URL。修改管理员令牌需重启进程；用户 Key 轮换即时生效。

## 用户与生效规则

1. 添加用户并设置额度。新 Key 只在创建或轮换成功后显示一次，关闭后无法再次查询；丢失时轮换。
2. 如果界面提示 Key 验证未开启，前往访问策略显式开启。添加用户本身不会改变全局验证方式。已有 Key 非空的 TOML 初始配置默认开启。
3. 将 Key 交给相应 Bot，使用源码客户端的 `api_key` 参数，或 Yellowstone 的 `x-token` / `x-api-key` metadata。不要把管理员令牌交给 Bot。
4. 每用户额度在该用户所有订阅间累计，另受每流与全局上游额度限制。允许账户更新是单用户明确授权。

修改用户 Key/额度/状态、删除用户会终止该用户现有订阅；其他用户保留。移除 IP 会终止来自该 IP 的订阅。切换全局 Key 验证会终止全部订阅。Bot 需要处理流结束并重新订阅。手动断开仅关闭指定流，用户仍可重新连接；需禁止重连时停用用户或移除 IP。TCP 连接可能保持 HTTP/2 空闲状态，因此 TCP 数不会因关闭 Subscribe 立即归零。

删除/停用最后一个 Key **不会关闭认证**，此时没有用户可以凭 Key 进入。IP 白名单为空则拒绝所有 gRPC 客户端。Key 验证关闭时，身份为 `ip:<来源IP>`，命名 Key 用户的额度不用于匿名 IP；已有 TOML 的 `grpc.shared.users."ip:..."` 配额可在面板编辑，不应向这类身份分配用户 Key。

## 持久化与备份

首次启动时从 TOML 的 `grpc.api_keys`、`grpc.allowed_sources`、`grpc.shared.users` 初始化。第一次成功保存管理修改后，`dashboard-state.toml` 成为这三类设置的权威来源，重启后仍然生效；原 TOML 不被改写。其他字段继续使用 TOML。

状态文件含用户明文 Key，必须是普通文件且 Unix 权限不向组或其他用户开放（推荐 `chmod 600 dashboard-state.toml`）。进程以原子替换方式保存；多个服务不能写同一状态文件。外部修改、文件损坏、权限不安全会导致拒绝保存或启动失败，不会自动回退到旧 Key。管理 API 查询不会返回这些密钥。

备份状态文件、实际配置、`.dashboard-token` 和隧道凭证，限制备份访问权限。安装升级会保留根目录 TOML、两种令牌和证书；自定义外部状态路径由运维自行备份。不要删除状态文件来“重置界面”：删除后重启会重新启用 TOML 中的旧用户/Key。停用面板或切回旧二进制同样需先核对 TOML 策略。

保存会校验页面看到的配置版本。多管理员同时编辑时，过时修改返回 409，刷新/重新打开表单后重试；不会默默覆盖其他管理员修改。请求中断或超时时保存可能已经完成，先刷新状态再决定是否重试。新建/轮换响应丢失时，可再次轮换生成一个已知 Key。

## 开发验证

```bash
cd dashboard
npm ci
npm run build
npm test
# 开发页面：127.0.0.1:5173，API 代理到已运行的 Rust 面板 127.0.0.1:8080
npm run dev
```

前端没有转发热路径逻辑，每 2 秒读取服务状态；文件持久化在后台阻塞任务进行，不在转发锁内执行。默认不开启面板时，不加载静态页面、不启动管理监听。没有进行生产流量延迟基准测试。

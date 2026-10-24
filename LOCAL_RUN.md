# 本地编译与启动 Hub（Mac + Docker / OrbStack）

先启动 Docker 或 OrbStack。以下命令在 Mac 项目终端执行，使用 Docker 内的 Rust 环境编译，无须在 Mac 安装 Cargo。

## 重新编译

```bash
cd /Users/lzj/code/monitor/monitor

# 首次使用或前端依赖有变化时执行
npm --prefix web-admin ci

npm --prefix web-admin run build

docker run --rm \
  -v "$PWD:/work" -w /work \
  rust:1-bookworm cargo build --release --locked
```

生成的 `target/release/monitor-hub` 是 Linux 程序，需要在 Docker 内运行。

## 首次启动（容器不存在时）

```bash
cd /Users/lzj/code/monitor/monitor

docker run -d \
  --name monitor-local \
  --restart unless-stopped \
  -p 28080:28080 \
  -v "$PWD:/work" \
  -w /work \
  rust:1-bookworm \
  ./target/release/monitor-hub 0.0.0.0:28080 --db monitor.db
```

监听所有 IPv4 地址的 `28080` 端口；本机访问 <http://localhost:28080>，局域网访问 `http://Mac的IP:28080`。

数据库保存在项目目录的 `monitor.db`，主题目录默认为项目下的 `themes/`。已有数据库会继续使用；容器停止、重启或重建不会删除挂载在项目目录中的数据。

## 更新代码后重启（容器已存在时）

先执行上面的重新编译命令，再执行：

```bash
docker restart monitor-local
```

重启会加载新编译的程序。若首次启动提示容器名称已存在，可先用下面的查看命令确认状态；已有容器直接启动或重启即可。

## 查看与停止

```bash
# 查看容器，包括已停止的容器
docker ps -a --filter name=monitor-local

# 查看最近日志；首次初始化数据库时，日志包含应急登录密码
docker logs --tail 50 monitor-local

# 持续查看日志，Ctrl+C 只退出日志查看
docker logs -f monitor-local

# 停止 / 启动已有容器
docker stop monitor-local
docker start monitor-local
```

## 公网域名与 Agent 下载验证

公网访问需将反向代理指向 Mac 的 `28080` 端口。若需要显式指定节点使用的对外地址，在首次启动命令的末尾追加：

```bash
--site https://r.lzja.cc:3000
```

已有容器修改启动参数，需要先停止并移除容器，再执行加好参数的完整 `docker run` 命令：

```bash
docker stop monitor-local
docker rm monitor-local
```

以上仅移除容器，项目目录中的数据库和主题保留。

Agent 签名版本发布成功，且 Hub 已重新编译加载对应公钥后，检查下载接口：

```bash
curl -sS -o /dev/null -w 'HTTP %{http_code}\n' \
  https://r.lzja.cc:3000/agent/x86_64
```

应返回 `HTTP 200`。若仍失败，查看具体错误（成功时下载内容保存到临时文件，不输出二进制到终端）：

```bash
curl -sS -o /tmp/monitor-agent-download -w 'HTTP %{http_code}\n' \
  https://r.lzja.cc:3000/agent/x86_64
file /tmp/monitor-agent-download
```

若 `file` 显示文本，可执行 `cat /tmp/monitor-agent-download` 查看错误。下载验证不会执行 Agent，也无需提供节点 Token。

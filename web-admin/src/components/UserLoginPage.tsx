import { useState } from "react"
import { Eye, EyeOff, LoaderCircle, LockKeyhole } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

type LoginError = "credentials" | "missing" | "network"

type UserLoginPageProps = {
  go: (path: string) => void
}

const LOGIN_ERRORS: Record<LoginError, string> = {
  credentials: "用户名或密码错误，请检查后重试。",
  missing: "该账号不存在，请确认用户名是否正确。",
  network: "网络连接失败，请检查网络后重试。",
}

export function UserLoginPage({ go }: UserLoginPageProps) {
  const [username, setUsername] = useState("")
  const [password, setPassword] = useState("")
  const [showPassword, setShowPassword] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<LoginError | null>(null)

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)
    setLoading(true)
    await new Promise<void>((resolve) => window.setTimeout(resolve, 450))
    setLoading(false)

    const mockState = new URLSearchParams(window.location.search).get("mock")
    if (mockState === "network") {
      setError("network")
    } else if (mockState === "missing") {
      setError("missing")
    } else if (mockState === "credentials") {
      setError("credentials")
    } else {
      go("/user")
    }
  }

  return (
    <main className="grid min-h-svh place-items-center bg-muted/30 p-4 sm:p-6">
      <section className="w-full max-w-[440px] space-y-6">
        <div className="flex flex-col items-center gap-2 text-center">
          <div className="grid size-12 place-items-center rounded-xl bg-primary text-lg font-bold text-primary-foreground shadow-sm">H</div>
          <div className="space-y-1">
            <p className="text-xl font-semibold tracking-tight">HHUB</p>
            <p className="text-sm text-muted-foreground">用户中心</p>
          </div>
        </div>

        <Card className="gap-0 overflow-hidden p-0">
          <div className="space-y-2 border-b px-6 py-5">
            <h1 className="text-lg font-semibold">登录用户中心</h1>
            <p className="text-sm text-muted-foreground">登录后查看流量、订阅地址及可用代理节点。</p>
          </div>
          <form className="space-y-5 p-6" onSubmit={(event) => void submit(event)} aria-busy={loading}>
            {error && (
              <p role="alert" className="rounded-md border border-destructive/20 bg-destructive/5 px-3 py-2.5 text-sm text-destructive">
                {LOGIN_ERRORS[error]}
              </p>
            )}

            <div className="space-y-2">
              <Label htmlFor="user-login-name">用户名</Label>
              <Input
                id="user-login-name"
                autoComplete="username"
                placeholder="请输入用户名"
                value={username}
                disabled={loading}
                required
                onChange={(event) => { setUsername(event.target.value); setError(null) }}
              />
            </div>

            <div className="space-y-2">
              <Label htmlFor="user-login-password">密码</Label>
              <div className="relative">
                <Input
                  id="user-login-password"
                  className="pr-11"
                  type={showPassword ? "text" : "password"}
                  autoComplete="current-password"
                  placeholder="请输入密码"
                  value={password}
                  disabled={loading}
                  required
                  onChange={(event) => { setPassword(event.target.value); setError(null) }}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground"
                  title={showPassword ? "隐藏密码" : "显示密码"}
                  aria-label={showPassword ? "隐藏密码" : "显示密码"}
                  aria-pressed={showPassword}
                  disabled={loading}
                  onClick={() => setShowPassword((visible) => !visible)}
                >
                  {showPassword ? <EyeOff /> : <Eye />}
                </Button>
              </div>
            </div>

            <Button className="w-full" type="submit" disabled={loading}>
              {loading ? <><LoaderCircle className="animate-spin" />正在登录…</> : <><LockKeyhole />登录</>}
            </Button>
            <p className="text-center text-xs text-muted-foreground">当前为页面预览，填写后将进入示例用户中心，认证服务尚未接入。</p>
          </form>
        </Card>

        <div className="text-center">
          <a className="text-sm text-muted-foreground transition-colors hover:text-foreground" href="/admin">
            进入管理后台 <span aria-hidden="true">→</span>
          </a>
        </div>
      </section>
    </main>
  )
}

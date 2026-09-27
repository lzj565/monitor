import { useEffect, useState } from "react"

import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Separator } from "@/components/ui/separator"
import { api } from "@/lib/api"

/** lucide removed brand marks in v1, and the GitHub logo is what makes this
 *  button recognisable at a glance. */
function GithubMark() {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden className="size-4">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
    </svg>
  )
}

// The hub redirects a failed GitHub sign-in back here with the reason attached,
// so it is readable in context rather than as a bare 401 page.
function callbackError(): string {
  const reason = new URLSearchParams(location.search).get("login_error")
  if (reason) history.replaceState({}, "", location.pathname)
  return reason ?? ""
}

export function Login({ github, userDefault, onModeChange, onDone }: {
  github: boolean
  userDefault: boolean
  onModeChange: (mode: "user" | "admin") => void
  onDone: (mode: "user" | "admin") => void
}) {
  const [mode, setMode] = useState<"user" | "admin">(() => userDefault ? "user" : "admin")
  const [username, setUsername] = useState("")
  const [password, setPassword] = useState("")
  const [error, setError] = useState(callbackError)
  const [busy, setBusy] = useState(false)

  useEffect(() => { setMode(userDefault ? "user" : "admin") }, [userDefault])

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setBusy(true)
    setError("")
    try {
      if (mode === "user") {
        await api("/user/auth/login", { method: "POST", body: JSON.stringify({ username, password }) })
      } else {
        await api("/admin/auth/login", { method: "POST", body: JSON.stringify({ password }) })
      }
      onDone(mode)
    } catch (err) {
      setError((err as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="grid min-h-svh place-items-center p-6">
      <Card className="w-full max-w-sm gap-5 p-6">
        <div className="grid grid-cols-2 rounded-lg bg-muted p-1" role="tablist" aria-label="登录身份">
          {(["user", "admin"] as const).map((next) => (
            <button
              key={next}
              type="button"
              role="tab"
              aria-selected={mode === next}
              onClick={() => {
                if (mode === next) return
                setMode(next)
                setPassword("")
                setError("")
                onModeChange(next)
              }}
              className={`rounded-md px-3 py-2 text-sm font-medium transition-colors ${mode === next ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground"}`}
            >
              {next === "user" ? "用户登录" : "管理员登录"}
            </button>
          ))}
        </div>

        {error && (
          <p className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</p>
        )}

        {mode === "admin" && github && (
          <>
            <Button asChild variant="outline" className="w-full">
              <a href="/api/auth/github">
                <GithubMark /> 使用 GitHub 登录
              </a>
            </Button>
            <div className="relative">
              <Separator />
              <span className="absolute inset-0 -top-2 mx-auto w-fit bg-card px-2 text-xs text-muted-foreground">
                或使用应急密码
              </span>
            </div>
          </>
        )}

        <form onSubmit={submit} className="space-y-3">
          {mode === "user" && (
            <div className="space-y-1.5">
              <Label htmlFor="username" className="text-xs">用户名</Label>
              <Input
                id="username"
                autoComplete="username"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                autoFocus
              />
            </div>
          )}
          <div className="space-y-1.5">
            <Label htmlFor="password" className="text-xs">{mode === "user" ? "密码" : "管理员密码"}</Label>
            <Input
              id="password"
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
              autoFocus={mode === "admin" && !github}
            />
          </div>
          <Button type="submit" className="w-full" disabled={busy || !password || (mode === "user" && !username.trim())}>
            {mode === "user" ? "登录用户中心" : "登录管理后台"}
          </Button>
        </form>
      </Card>
    </div>
  )
}

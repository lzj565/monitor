import { Activity, ArrowLeft, LogOut, Moon, Sun } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import type { UserCenterData } from "@/lib/user-center-mock"

type UserCenterHeaderProps = {
  user: UserCenterData["user"]
  dark: boolean
  toggleTheme: () => void
  onLogout: () => void
  onReturnToAdmin: () => void
}

export function UserCenterHeader({ user, dark, toggleTheme, onLogout, onReturnToAdmin }: UserCenterHeaderProps) {
  return (
    <header className="border-b bg-background">
      <div className="mx-auto flex max-w-7xl flex-wrap items-center gap-3 px-4 py-3 sm:px-6 lg:px-8">
        <div className="flex items-center gap-3">
          <span className="grid size-9 place-items-center rounded-lg bg-primary text-sm font-bold text-primary-foreground">H</span>
          <span className="font-semibold tracking-tight">HHUB</span>
          <Badge variant="secondary" className="font-normal">用户中心</Badge>
        </div>

        <div className="ml-auto flex flex-wrap items-center justify-end gap-1.5 sm:gap-2">
          {user.impersonation && <Badge variant="outline" className="border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-300">管理员预览</Badge>}
          {user.impersonation && (
            <Button size="sm" variant="outline" onClick={onReturnToAdmin}>
              <ArrowLeft /> 返回管理后台
            </Button>
          )}
          <Button size="sm" variant="outline" asChild>
            <a href="/" target="_blank" rel="noreferrer">
              <Activity /> 服务器探针
            </a>
          </Button>
          <span className="hidden px-2 text-sm text-muted-foreground sm:inline">用户：{user.username}</span>
          <Button size="icon" variant="ghost" title="切换主题" aria-label="切换主题" onClick={toggleTheme}>
            {dark ? <Sun /> : <Moon />}
          </Button>
          <Button size="sm" variant="ghost" title="退出登录" onClick={onLogout}>
            <LogOut /> <span className="hidden sm:inline">退出</span>
          </Button>
        </div>
      </div>
    </header>
  )
}

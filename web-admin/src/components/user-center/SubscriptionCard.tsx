import { useState } from "react"
import { QRCodeSVG } from "qrcode.react"
import { Copy, QrCode, RotateCcw, Zap } from "lucide-react"
import { toast } from "sonner"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { ClientImportCard } from "@/components/user-center/ClientImportCard"

export function SubscriptionCard({ initialUrl }: { initialUrl: string }) {
  const [url, setUrl] = useState(initialUrl)
  const [qrOpen, setQrOpen] = useState(false)
  const [resetOpen, setResetOpen] = useState(false)

  async function copyForClient(clientName?: string) {
    try {
      await navigator.clipboard.writeText(url)
      toast.success(clientName ? `${clientName} 订阅地址已复制` : "订阅地址已复制")
    } catch {
      toast.error("复制失败，请手动选择订阅地址")
    }
  }

  function resetLink() {
    const token = `demo-${Date.now().toString(36)}`
    setUrl(`https://example.com/api/proxy/sub?token=${token}`)
    setResetOpen(false)
    toast.success("演示订阅链接已更新")
  }

  return (
    <>
      <Card className="gap-0 overflow-hidden py-0">
        <div className="space-y-2 border-b px-5 py-4 sm:px-6">
          <div className="flex flex-wrap items-center gap-2">
            <Zap className="size-5 text-amber-600" />
            <h2 className="text-lg font-semibold tracking-tight">一键聚合订阅</h2>
            <Badge variant="secondary" className="font-normal">演示数据</Badge>
          </div>
          <p className="max-w-4xl text-sm text-muted-foreground">采用智能 UA 自适应技术，单一链接自动适配 Clash、Sing-box、Shadowrocket、v2rayN 等主流客户端。</p>
        </div>

        <div className="space-y-5 p-5 sm:p-6">
          <div className="space-y-2">
            <label className="text-sm font-medium" htmlFor="user-subscription-url">订阅地址</label>
            <div className="flex flex-col gap-2 md:flex-row">
              <Input
                id="user-subscription-url"
                className="min-w-0 font-mono text-xs sm:text-sm"
                value={url}
                readOnly
                onFocus={(event) => event.currentTarget.select()}
              />
              <div className="grid grid-cols-2 gap-2 sm:flex">
                <Button size="sm" onClick={() => void copyForClient()}><Copy />复制订阅</Button>
                <Button size="sm" variant="outline" onClick={() => setQrOpen(true)}><QrCode />扫码导入</Button>
                <Button size="sm" variant="secondary" className="col-span-2 sm:col-span-1" onClick={() => setResetOpen(true)}><RotateCcw />重置链接</Button>
              </div>
            </div>
          </div>

          <ClientImportCard onUse={(clientName) => void copyForClient(clientName)} />
        </div>
      </Card>

      <Dialog open={qrOpen} onOpenChange={setQrOpen}>
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>扫码导入订阅</DialogTitle>
            <DialogDescription>使用客户端扫描二维码添加聚合订阅。</DialogDescription>
          </DialogHeader>
          <div className="mx-auto rounded-xl border bg-white p-4">
            <QRCodeSVG value={url} size={220} level="M" bgColor="#ffffff" fgColor="#111827" />
          </div>
          <p className="break-all text-center font-mono text-xs text-muted-foreground">{url}</p>
        </DialogContent>
      </Dialog>

      <AlertDialog open={resetOpen} onOpenChange={setResetOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>重置订阅链接？</AlertDialogTitle>
            <AlertDialogDescription>重置后旧订阅链接将立即失效，需要重新导入所有客户端。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction onClick={resetLink}>确认重置</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

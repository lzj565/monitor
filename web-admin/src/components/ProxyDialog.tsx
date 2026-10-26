import type { ReactNode } from "react"
import { Dialog, DialogContent } from "@/components/ui/dialog"
export const PROXY_DIALOG_CLASS = "flex max-h-[90svh] w-full max-w-[calc(100%-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-xl"
export function ProxyDialog({ children, onClose, busy = false, compact = false }: { children: ReactNode; onClose: () => void; busy?: boolean; compact?: boolean }) {
  return <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose() }}><DialogContent className={`${PROXY_DIALOG_CLASS}${compact ? " sm:max-w-sm" : ""}`}>{children}</DialogContent></Dialog>
}

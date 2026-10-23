import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { api } from "@/lib/api"

export type Settings = Record<string, string | boolean | number>

// Two pages write settings, and each loads only what it displays.
export function useSettings() {
  const [s, setS] = useState<Settings | null>(null)
  useEffect(() => { api<Settings>("/settings").then(setS).catch(() => {}) }, [])
  return {
    s,
    set: (k: string, v: string) => setS((old) => ({ ...(old ?? {}), [k]: v })),
    // Resolves to whether the hub took the patch; a failure is already toasted.
    save: async (patch: Record<string, string>, done = "已保存") => {
      try {
        await api("/settings", { method: "PUT", body: JSON.stringify(patch) })
      } catch (e) {
        toast.error((e as Error).message)
        return false
      }
      toast.success(done)
      // Only the saved keys and the `*_set` flags are taken from the hub: a
      // credential comes back as a flag, so the typed value must not linger,
      // while another card's unsaved edits on the same page must survive. A
      // failed read leaves the form as typed; the save itself stands.
      try {
        const fresh = await api<Settings>("/settings")
        setS((old) => {
          const next = { ...old }
          for (const key of [...Object.keys(patch), "your_address"]) next[key] = fresh[key]
          for (const [key, value] of Object.entries(fresh)) if (key.endsWith("_set")) next[key] = value
          return next
        })
      } catch (e) {
        toast.error((e as Error).message)
      }
      return true
    },
  }
}

export type Versions = { hub: string; hub_latest: string; agent_latest: string; notice: boolean }

/**
 * What is running and what is published. Read once per panel load: the hub holds
 * the answer for six hours, so this costs a GitHub lookup a few times a day at
 * most, and nothing at all while nobody opens the panel.
 *
 * A hub that cannot reach github.com answers with empty latest fields, which
 * render as no update rather than as an error.
 */
export function useVersions() {
  const [versions, setVersions] = useState<Versions | null>(null)
  const load = useCallback(() => api<Versions>("/version").then(setVersions).catch(() => {}), [])
  useEffect(() => { load() }, [load])
  return { versions, reload: load }
}



const ICON_BYTES = 20 * 1024


const decodedBytes = (url: string) => Math.floor(((url.length - url.indexOf(",") - 1) * 3) / 4)


function drawIcon(img: HTMLImageElement, side: number, background: string | null, type: string, quality?: number) {
  const canvas = document.createElement("canvas")
  canvas.width = canvas.height = side
  const context = canvas.getContext("2d")!
  if (background) {
    context.fillStyle = background
    context.fillRect(0, 0, side, side)
  }
  // An SVG without width and height has no intrinsic size; drawn square.
  const w = img.naturalWidth || side
  const h = img.naturalHeight || side
  const scale = side / Math.max(w, h)
  context.drawImage(img, (side - w * scale) / 2, (side - h * scale) / 2, w * scale, h * scale)
  return canvas.toDataURL(type, quality)
}


export async function siteIcons(file: File) {
  const url = URL.createObjectURL(file)
  try {
    const img = new Image()
    img.src = url
    await img.decode().catch(() => {
      throw new Error("这张图片打不开，换一张 PNG、SVG 或 ICO 试试")
    })
    const favicon =
      file.type === "image/svg+xml" && file.size <= ICON_BYTES
        ? await new Promise<string>((resolve, reject) => {
            const reader = new FileReader()
            reader.onload = () => resolve(String(reader.result))
            reader.onerror = () => reject(new Error("读取图片失败，请重新选择"))
            reader.readAsDataURL(file)
          })
        : drawIcon(img, 64, null, "image/png")
    let touch = drawIcon(img, 180, "#fff", "image/png")
    for (let quality = 0.9; decodedBytes(touch) > ICON_BYTES && quality > 0.3; quality -= 0.15) {
      touch = drawIcon(img, 180, "#fff", "image/jpeg", quality)
    }
    return { favicon, touch_icon: touch }
  } finally {
    URL.revokeObjectURL(url)
  }
}

/** Form input uses GiB; API values are whole bytes and device counts. */
export function parseUserLimits(traffic: string, devices: string) {
  const gib = Number(traffic)
  const traffic_limit = Math.round(gib * 1024 ** 3)
  const device_limit = Number(devices)
  if (!Number.isFinite(gib) || gib < 0 || !Number.isSafeInteger(traffic_limit)) {
    throw new Error("请填写有效的非负流量限额")
  }
  if (!Number.isSafeInteger(device_limit) || device_limit < 0) {
    throw new Error("设备数必须为非负整数")
  }
  return { traffic_limit, device_limit }
}

export function parseResetDay(value: string): number {
  if (!value.trim()) return 0
  const day = Number(value)
  if (!Number.isInteger(day) || day < 1 || day > 31) throw new Error("流量重置日必须为 1–31 的整数，或留空")
  return day
}

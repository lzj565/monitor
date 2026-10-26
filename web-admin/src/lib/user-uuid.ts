export function normalizeUserUuid(value: string): string {
  const uuid = value.trim().toLowerCase()
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(uuid) || uuid === "00000000-0000-0000-0000-000000000000") throw new Error("请输入有效的非全零 UUID（8-4-4-4-12 格式）")
  return uuid
}

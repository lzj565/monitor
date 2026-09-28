export type ParsedSni = { server_name: string; server_port: number }
export type ParsedDestination = { server: string; server_port: number }

/** Parse the modal's host[:port] SNI value into the existing Reality fields. */
export function parseSni(value: string): ParsedSni {
  const input = value.trim()
  if (!input) throw new Error("请填写 SNI")

  let host = input
  let portText = "443"
  if (input.startsWith("[")) {
    const closing = input.indexOf("]")
    if (closing < 2) throw new Error("IPv6 SNI 请使用 [地址]:端口 格式")
    host = input.slice(1, closing)
    const suffix = input.slice(closing + 1)
    if (suffix && !suffix.startsWith(":")) throw new Error("SNI 格式应为 host:port")
    if (suffix.startsWith(":")) portText = suffix.slice(1)
  } else {
    const firstColon = input.indexOf(":")
    const lastColon = input.lastIndexOf(":")
    if (firstColon !== lastColon) throw new Error("IPv6 SNI 请使用 [地址]:端口 格式")
    if (firstColon >= 0) {
      host = input.slice(0, firstColon)
      portText = input.slice(firstColon + 1)
    }
  }

  const server_port = Number(portText)
  if (!host || !/^\d+$/.test(portText) || !Number.isInteger(server_port) || server_port < 1 || server_port > 65535) {
    throw new Error("SNI 端口必须在 1 到 65535 之间")
  }
  return { server_name: host, server_port }
}

export function formatSni(serverName: string, serverPort: number): string {
  const host = serverName.includes(":") && !serverName.startsWith("[") ? `[${serverName}]` : serverName
  return `${host}:${serverPort}`
}

/** Parse Reality's independent handshake destination in host[:port] form. */
export function parseDestination(value: string): ParsedDestination {
  const parsed = parseSni(value)
  return { server: parsed.server_name, server_port: parsed.server_port }
}

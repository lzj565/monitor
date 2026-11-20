export function normaliseRoute(path: string, search: string) {
  let next = path.replace(/\/$/, "") || "/admin/nodes"
  if (next === "/admin") next = "/admin/nodes"
  if (next === "/user") next = "/user/center"
  if (next === "/admin/proxies" && new URLSearchParams(search).get("type") === "external") {
    const params = new URLSearchParams(search)
    params.delete("type")
    return { path: "/admin/external-proxies", search: params.size ? `?${params}` : "" }
  }
  return { path: next, search }
}

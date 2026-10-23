export function normaliseRoute(path: string, search: string) {
  let next = path.replace(/\/$/, "") || "/admin/nodes"
  if (next === "/admin") next = "/admin/nodes"
  if (next === "/user") next = "/user/center"
  if (next === "/admin/realm") {
    const params = new URLSearchParams(search)
    params.set("type", "realm")
    return { path: "/admin/proxies", search: `?${params}` }
  }
  return { path: next, search }
}

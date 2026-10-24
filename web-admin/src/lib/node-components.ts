import { api } from "./api"
export type InstallChoices = { singbox: true; realm: boolean; mieru: boolean }
export type ComponentState = { component: "singbox" | "realm" | "mieru"; desired_installed: boolean; installed: boolean; managed: boolean; running: boolean; version: string; checked_at: number | null; last_error: string }
export type ComponentProfile = { install: InstallChoices; components: ComponentState[]; online: boolean }
export function getComponents(id: number, signal?: AbortSignal) { return api<ComponentProfile>(`/nodes/${id}/components`, { signal, cache: "no-store" }) }
export function saveComponents(id: number, install: InstallChoices) { return api<ComponentProfile>(`/nodes/${id}/components`, { method: "PUT", body: JSON.stringify(install) }) }
export function sameInstall(a: InstallChoices, b: InstallChoices) { return a.singbox === b.singbox && a.realm === b.realm && a.mieru === b.mieru }

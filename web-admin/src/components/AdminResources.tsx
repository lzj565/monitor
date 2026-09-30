import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react"
import type { ReactNode } from "react"
import { createResourceCache } from "@/lib/resource-cache"
import { listAllProxies, listUsers } from "@/lib/resources"

function createResources() {
  return { proxies: createResourceCache(listAllProxies), users: createResourceCache(listUsers) }
}

const Context = createContext<(ReturnType<typeof createResources> & {
  authorizationRevision: number
  invalidateAuthorizations: () => void
}) | null>(null)

export function AdminResourcesProvider({ nodes, children }: { nodes: { id: number }[]; children: ReactNode }) {
  const [resources] = useState(createResources)
  const [authorizationRevision, setAuthorizationRevision] = useState(0)
  const invalidateAuthorizations = useCallback(() => setAuthorizationRevision((value) => value + 1), [])
  const nodeKey = nodes.map((node) => node.id).sort((a, b) => a - b).join(",")
  const previousNodeKey = useRef(nodeKey)
  useEffect(() => {
    void resources.users.refresh()
    return () => {
      resources.users.cancel()
      resources.proxies.cancel()
    }
  }, [resources])
  useEffect(() => {
    const changed = previousNodeKey.current !== nodeKey
    previousNodeKey.current = nodeKey
    void resources.proxies.refresh(changed)
    if (changed) {
      invalidateAuthorizations()
      void resources.users.refresh(true)
    }
  }, [nodeKey, resources, invalidateAuthorizations])
  const value = useMemo(() => ({ ...resources, authorizationRevision, invalidateAuthorizations }), [resources, authorizationRevision, invalidateAuthorizations])
  return <Context.Provider value={value}>{children}</Context.Provider>
}

export function useAdminResources() {
  const resources = useContext(Context)
  if (!resources) throw new Error("Administrator resource provider is missing")
  return resources
}

function useCachedList<T>(cache: ReturnType<typeof createResourceCache<T>>) {
  const snapshot = useSyncExternalStore(cache.subscribe, cache.getSnapshot)
  useEffect(() => { void cache.refresh() }, [cache])
  const reload = useCallback(() => { void cache.refresh() }, [cache])
  return { ...snapshot, loading: snapshot.items === null && !snapshot.error, reload }
}

export function useSharedProxies() {
  return useCachedList(useAdminResources().proxies)
}

export function useSharedUsers() {
  return useCachedList(useAdminResources().users)
}

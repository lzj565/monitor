export type ResourceSnapshot<T> = {
  items: T[] | null
  error: string
  refreshing: boolean
}

// One cache per administrator session. Refreshes retain data and obsolete
// responses cannot overwrite a refresh started after a successful write.
export function createResourceCache<T>(fetchItems: () => Promise<T[]>) {
  let snapshot: ResourceSnapshot<T> = { items: null, error: "", refreshing: false }
  let generation = 0
  let pending: Promise<void> | null = null
  const listeners = new Set<() => void>()
  const publish = (next: ResourceSnapshot<T>) => {
    snapshot = next
    listeners.forEach((listener) => listener())
  }
  const refresh = (force = false): Promise<void> => {
    if (pending && !force) return pending
    const current = ++generation
    publish({ ...snapshot, error: "", refreshing: true })
    const request = Promise.resolve().then(fetchItems).then((items) => {
      if (generation === current) publish({ items, error: "", refreshing: false })
    }, (error: unknown) => {
      if (generation === current) publish({ ...snapshot, error: error instanceof Error ? error.message : String(error), refreshing: false })
    }).finally(() => {
      if (generation === current) pending = null
    })
    pending = request
    return request
  }
  return {
    getSnapshot: () => snapshot,
    subscribe: (listener: () => void) => {
      listeners.add(listener)
      return () => { listeners.delete(listener) }
    },
    refresh,
    cancel: () => {
      generation += 1
      pending = null
    },
  }
}

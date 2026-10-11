// Legacy commands omit this option; explicit false requests uninstall.
export function realmArg(realm: boolean | undefined): string[] {
  return realm === undefined ? [] : [realm ? "--realm" : "--no-realm"]
}

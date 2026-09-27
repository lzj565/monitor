import { x25519 } from "@noble/curves/ed25519.js"

export type RealityKeyPair = { privateKey: string; publicKey: string }

function base64Url(bytes: Uint8Array): string {
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "")
}

function fromBase64Url(value: string): Uint8Array {
  const base64 = value.replace(/-/g, "+").replace(/_/g, "/")
  const binary = atob(base64 + "=".repeat((4 - base64.length % 4) % 4))
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export function generateRealityKeyPair(): RealityKeyPair {
  const pair = x25519.keygen()
  return { privateKey: base64Url(pair.secretKey), publicKey: base64Url(pair.publicKey) }
}

export function realityKeyPairMatches(privateKey: string, publicKey: string): boolean {
  try {
    const secret = fromBase64Url(privateKey)
    return secret.length === 32 && base64Url(x25519.getPublicKey(secret)) === publicKey
  } catch {
    return false
  }
}

export function generateShortId(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(4)), (byte) => byte.toString(16).padStart(2, "0")).join("")
}

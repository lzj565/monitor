export type UserCenterStatus = "active" | "exhausted" | "expired" | "disabled"

export type UserCenterNode = {
  id: number
  name: string
  region: string
  flag: string
  address: string
  port: number
  protocol: string
  ipVersion: "IPv4" | "IPv6"
  online: boolean
}

export type UserCenterData = {
  user: {
    username: string
    status: UserCenterStatus
    impersonation: boolean
  }
  traffic: {
    uploadBytes: number
    downloadBytes: number
    usedBytes: number
    limitBytes: number
    resetAt: string | null
  }
  devices: {
    online: number
    limit: number
  }
  expireAt: string | null
  subscriptionUrl: string
  nodes: UserCenterNode[]
}

const DEMO_LIMIT_BYTES = 200 * 1024 ** 3

const DEMO_USER_CENTER: UserCenterData = {
  user: {
    username: "aaa",
    status: "active",
    impersonation: false,
  },
  traffic: {
    uploadBytes: 0,
    downloadBytes: 0,
    usedBytes: 0,
    limitBytes: DEMO_LIMIT_BYTES,
    resetAt: null,
  },
  devices: {
    online: 0,
    limit: 0,
  },
  expireAt: "2026-10-25T08:00:00+08:00",
  subscriptionUrl: "https://example.com/api/proxy/sub?token=demo-subscription-token",
  nodes: [
    {
      id: 1,
      name: "[HK服务器] Reality",
      region: "HK",
      flag: "🇭🇰",
      address: "82.123.45.116",
      port: 24060,
      protocol: "VLESS REALITY",
      ipVersion: "IPv4",
      online: true,
    },
  ],
}

function isUserCenterStatus(value: string | null): value is UserCenterStatus {
  return value === "active" || value === "exhausted" || value === "expired" || value === "disabled"
}

export function getUserCenterMock(search: string): UserCenterData {
  const params = new URLSearchParams(search)
  const requestedStatus = params.get("status")
  const status = isUserCenterStatus(requestedStatus) ? requestedStatus : DEMO_USER_CENTER.user.status
  const unlimited = params.get("quota") === "unlimited"
  const limitBytes = unlimited ? 0 : DEMO_USER_CENTER.traffic.limitBytes
  const usedBytes = status === "exhausted" && !unlimited ? limitBytes : DEMO_USER_CENTER.traffic.usedBytes
  const uploadBytes = status === "exhausted" ? usedBytes / 4 : DEMO_USER_CENTER.traffic.uploadBytes
  const downloadBytes = status === "exhausted" ? usedBytes - uploadBytes : DEMO_USER_CENTER.traffic.downloadBytes

  return {
    ...DEMO_USER_CENTER,
    user: {
      ...DEMO_USER_CENTER.user,
      status,
      impersonation: params.get("preview") === "1",
    },
    traffic: {
      ...DEMO_USER_CENTER.traffic,
      uploadBytes,
      downloadBytes,
      usedBytes,
      limitBytes,
    },
    expireAt: status === "expired"
      ? "2025-10-25T08:00:00+08:00"
      : params.get("expiry") === "never"
        ? null
        : DEMO_USER_CENTER.expireAt,
  }
}

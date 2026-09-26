import { UserCenterHeader } from "@/components/user-center/UserCenterHeader"
import { ProxyNodeList } from "@/components/user-center/ProxyNodeList"
import { SubscriptionCard } from "@/components/user-center/SubscriptionCard"
import { TrafficQuotaCard } from "@/components/user-center/TrafficQuotaCard"
import { getUserCenterMock } from "@/lib/user-center-mock"

type UserCenterPageProps = {
  dark: boolean
  toggleTheme: () => void
  go: (path: string) => void
  search: string
}

export function UserCenterPage({ dark, toggleTheme, go, search }: UserCenterPageProps) {
  const data = getUserCenterMock(search)

  return (
    <div className="min-h-svh bg-muted/20">
      <UserCenterHeader
        user={data.user}
        dark={dark}
        toggleTheme={toggleTheme}
        onLogout={() => go("/user/login")}
        onReturnToAdmin={() => go("/admin/proxy/users")}
      />
      <main className="mx-auto max-w-7xl space-y-6 px-4 py-6 sm:px-6 sm:py-8 lg:px-8">
        <TrafficQuotaCard data={data} />
        <SubscriptionCard initialUrl={data.subscriptionUrl} />
        <ProxyNodeList nodes={data.nodes} />
      </main>
    </div>
  )
}

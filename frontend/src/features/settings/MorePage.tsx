import { AutoOpenPanel, type AutoOpenMutationLease } from '../calls/AutoOpenPanel'
import type { AutoOpenPolicy, CallApi } from '../calls/callApi'

export interface MorePageProps {
  autoOpenMutationBusy?: boolean
  autoOpenPolicy?: AutoOpenPolicy | null
  callsApi: CallApi
  getAutoOpenPolicyGeneration?: () => number
  isAutoOpenMutationCurrent?: (lease: AutoOpenMutationLease) => boolean
  onAutoOpenMutationEnd?: (lease: AutoOpenMutationLease) => void
  onAutoOpenMutationStart?: (controller: AbortController) => AutoOpenMutationLease | null
  onAutoOpenPolicyMutationConfirmed?: (lease: AutoOpenMutationLease) => void
  onOpenAuth(): void
  onPolicyChange(policy: AutoOpenPolicy): void
  connectionIssue?: string
}

export function MorePage({
  autoOpenMutationBusy,
  autoOpenPolicy,
  callsApi,
  getAutoOpenPolicyGeneration,
  isAutoOpenMutationCurrent,
  onAutoOpenMutationEnd,
  onAutoOpenMutationStart,
  onAutoOpenPolicyMutationConfirmed,
  onOpenAuth,
  onPolicyChange,
  connectionIssue,
}: MorePageProps) {
  return <div className="more-page">
    <header className="screen-heading"><h1>Ещё</h1></header>
    <button className="settings-row" onClick={onOpenAuth} type="button">MTS ID и подключение</button>
    {connectionIssue && <p className="connection-issue" role="status">{connectionIssue}</p>}
    <AutoOpenPanel
      api={callsApi}
      getPolicyGeneration={getAutoOpenPolicyGeneration}
      isMutationCurrent={isAutoOpenMutationCurrent}
      mutationBusy={autoOpenMutationBusy}
      onMutationEnd={onAutoOpenMutationEnd}
      onMutationStart={onAutoOpenMutationStart}
      onPolicyMutationConfirmed={onAutoOpenPolicyMutationConfirmed}
      onPolicyChange={onPolicyChange}
      policy={autoOpenPolicy}
    />
  </div>
}

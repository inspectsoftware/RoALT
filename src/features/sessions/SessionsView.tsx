import "../dashboard/dashboard.css";
import "./sessions.css";
import { useMemo, useState } from "react";
import { asError, type AccountSummary, type SessionConfig } from "../../api";
import { t } from "../../i18n";
import { Skeleton } from "../../ui/Bits";
import { Button } from "../../ui/Button";
import { friendly } from "../../ui/Toast";
import { useNow } from "../launch/Running";
import { PushMacrosDialog, RemoveDialog, SessionDialog } from "./Dialogs";
import { HealthPanel } from "./HealthPanel";
import { useSessions } from "./queries";
import { SessionPanel } from "./SessionPanel";

/** Sessions: background Windows sessions (loopback RDP) where clients run without taking the
 * focus. The setup check, then one panel per session with its clients. */
export function SessionsView({ accounts }: { accounts: AccountSummary[] }) {
  const sessions = useSessions();
  const now = useNow(30_000);
  const [dialog, setDialog] = useState<{ editing: SessionConfig | null } | null>(null);
  const [removing, setRemoving] = useState<SessionConfig | null>(null);
  const [pushing, setPushing] = useState<string | null>(null);
  const byId = useMemo(() => new Map(accounts.map((a) => [a.id, a])), [accounts]);
  const list = sessions.data ?? [];
  const configs = list.map((s) => s.config);

  return (
    <main className="scroll flex-1 min-w-0 page">
      <div className="ses-page">
        <header className="ses-page-head">
          <div className="min-w-0 flex-[1_1_320px]">
            <h1 className="page-title">{t("app.nav.sessions")}</h1>
            <p className="page-sub max-w-[72ch]">{t("sessions.lead")}</p>
          </div>
          <Button variant="primary" icon="plus" onClick={() => setDialog({ editing: null })}>
            {t("sessions.add")}
          </Button>
        </header>

        <HealthPanel names={configs.map((c) => c.name)} />

        {sessions.isError ? (
          <div role="alert" className="notice" data-tone="attention">
            <p className="alert flex-[1_1_240px] min-w-0">{friendly(asError(sessions.error))}</p>
            <Button size="md" icon="refresh" busy={sessions.isFetching} onClick={() => void sessions.refetch()}>{t("common.tryAgain")}</Button>
          </div>
        ) : !sessions.data ? (
          <div className="panel ses-panel">
            <Skeleton width={180} height={18} />
            <Skeleton width="50%" height={12} />
            <span className="sr-only">{t("common.loading")}</span>
          </div>
        ) : list.length === 0 ? (
          <div className="empty">
            <p>{t("sessions.empty")}</p>
            <Button variant="primary" icon="plus" onClick={() => setDialog({ editing: null })}>{t("sessions.add")}</Button>
          </div>
        ) : (
          list.map((s) => (
            <SessionPanel
              key={s.config.name}
              s={s}
              accounts={byId}
              now={now}
              onEdit={(c) => setDialog({ editing: c })}
              onRemove={setRemoving}
              onPushMacros={setPushing}
            />
          ))
        )}
      </div>

      <SessionDialog open={dialog !== null} editing={dialog?.editing ?? null} sessions={configs} onClose={() => setDialog(null)} />
      <RemoveDialog session={removing} onClose={() => setRemoving(null)} />
      <PushMacrosDialog session={pushing} onClose={() => setPushing(null)} />
    </main>
  );
}

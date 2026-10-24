// TanStack Query wiring: one cache, keys, and the hooks the views share.
import { MutationCache, QueryClient, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type AccountPatch, type AccountSummary, type Group, type Settings, type SettingsPatch } from "./api";
import { toastError } from "./ui/Toast";

export const qk = {
  vault: ["vault"],
  settings: ["settings"],
  accounts: ["accounts"],
  groups: ["groups"],
} as const;

export const queryClient = new QueryClient({
  // Data only changes through our own mutations and backend events, which invalidate.
  defaultOptions: { queries: { staleTime: Infinity, retry: false, refetchOnWindowFocus: false } },
  // Every failed mutation toasts unless it handles its own errors (meta.quiet).
  mutationCache: new MutationCache({
    onError: (e, _vars, _res, m) => {
      if (!m.meta?.quiet) toastError(e);
    },
  }),
});

const bySortOrder = <T extends { sortOrder: number }>(list: T[]) => [...list].sort((a, b) => a.sortOrder - b.sortOrder);

export const useVault = () => useQuery({ queryKey: qk.vault, queryFn: api.vaultStatus });
export const useSettings = () => useQuery({ queryKey: qk.settings, queryFn: api.settingsGet });
export const useAccounts = () => useQuery({ queryKey: qk.accounts, queryFn: api.accountsList, select: bySortOrder });
export const useGroups = () => useQuery({ queryKey: qk.groups, queryFn: api.groupsList, select: bySortOrder });

export function setAccounts(qc: ReturnType<typeof useQueryClient>, f: (list: AccountSummary[]) => AccountSummary[]) {
  qc.setQueryData<AccountSummary[]>(qk.accounts, (old) => (old ? f(old) : old));
}

/** Optimistic account edit; the server's copy replaces it on success. */
export function useUpdateAccount() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, patch }: { id: number; patch: AccountPatch }) => api.accountUpdate(id, patch),
    onMutate: ({ id, patch }) =>
      setAccounts(qc, (list) =>
        list.map((a) =>
          a.id === id ? { ...a, ...patch, color: patch.color === undefined ? a.color : patch.color || null } : a,
        ),
      ),
    onSuccess: (acc) => setAccounts(qc, (list) => list.map((a) => (a.id === acc.id ? acc : a))),
    onError: () => qc.invalidateQueries({ queryKey: qk.accounts }),
  });
}

/** Optimistic settings change (theme and density apply at once). */
export function useSetSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (patch: SettingsPatch) => api.settingsSet(patch),
    onMutate: (patch) => qc.setQueryData<Settings>(qk.settings, (s) => (s ? { ...s, ...patch } : s)),
    onSuccess: (s) => {
      qc.setQueryData(qk.settings, s);
      qc.invalidateQueries({ queryKey: qk.vault });
    },
    onError: () => qc.invalidateQueries({ queryKey: qk.settings }),
  });
}

export function useSaveGroup() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: api.groupSave,
    onSuccess: (g: Group) =>
      qc.setQueryData<Group[]>(qk.groups, (list) => (list ? [...list.filter((x) => x.id !== g.id), g] : list)),
  });
}

export const groupColor = (g: Group) => g.color ?? "var(--grey)";

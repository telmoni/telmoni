import type { StateCreator } from "zustand";

import type { AppStore, AuditExportsSlice } from "../types";

const NONE = {} as const;

export const createAuditExportsSlice = (): StateCreator<
  AppStore,
  [],
  [],
  AuditExportsSlice
> => (set, get) => ({
  auditExports: NONE,
  auditExportsStarted: 0,

  // Decided against the store's own state, inside the store, so no render
  // between the start and the read's answer can hide the start from it.
  replaceAuditExports: (organizationId, exports, startedBefore) => {
    const s = get();
    if (s.auditExportsStarted !== startedBefore) return null;
    const before = s.auditExports[organizationId] ?? [];
    set({ auditExports: { ...s.auditExports, [organizationId]: exports } });
    return before;
  },

  addAuditExport: (organizationId, entry) =>
    set((s) => ({
      auditExportsStarted: s.auditExportsStarted + 1,
      auditExports: {
        ...s.auditExports,
        [organizationId]: [
          entry,
          ...(s.auditExports[organizationId] ?? []).filter((e) => e.id !== entry.id),
        ],
      },
    })),

  markAuditExportDownloaded: (id) =>
    set((s) => ({
      auditExports: Object.fromEntries(
        Object.entries(s.auditExports).map(([organizationId, exports]) => [
          organizationId,
          exports.map((e) =>
            e.id === id && e.downloaded_at === null
              ? { ...e, downloaded_at: new Date().toISOString() }
              : e,
          ),
        ]),
      ),
    })),

  forgetAuditExport: (id) =>
    set((s) => ({
      auditExports: Object.fromEntries(
        Object.entries(s.auditExports).map(([organizationId, exports]) => [
          organizationId,
          exports.filter((e) => e.id !== id),
        ]),
      ),
    })),
});

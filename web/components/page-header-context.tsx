"use client";

import {
  createContext,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";

interface Header {
  backHref?: string;
  backLabel?: string;
}

type HeaderUpdater = Header | null | ((prev: Header | null) => Header | null);
type SetHeader = (next: HeaderUpdater) => void;

const HeaderValueCtx = createContext<Header | null>(null);
const HeaderSetCtx = createContext<SetHeader | null>(null);

export function PageHeaderProvider({ children }: { children: ReactNode }) {
  const [header, set] = useState<Header | null>(null);
  const setter = useMemo(() => set, []);
  return (
    <HeaderSetCtx.Provider value={setter}>
      <HeaderValueCtx.Provider value={header}>
        {children}
      </HeaderValueCtx.Provider>
    </HeaderSetCtx.Provider>
  );
}

export function usePageHeaderValue(): Header | null {
  return useContext(HeaderValueCtx);
}

export function usePageHeaderSet(): SetHeader {
  const set = useContext(HeaderSetCtx);
  if (!set) {
    throw new Error(
      "usePageHeaderSet must be used inside <PageHeaderProvider>",
    );
  }
  return set;
}

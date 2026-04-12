import type { ReactNode } from "react";

interface Props {
  children: ReactNode;
}

export function AppShell({ children }: Props) {
  return (
    <div className="flex h-full w-full overflow-hidden bg-surface">
      {children}
    </div>
  );
}

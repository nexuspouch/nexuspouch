import type { ReactNode } from 'react';

export type PillTone = 'default' | 'accent' | 'ok' | 'warn' | 'danger';

export function StatusPill({
  children,
  tone = 'default',
}: {
  children: ReactNode;
  tone?: PillTone;
}) {
  return <span className={`badge status-pill tone-${tone}`}>{children}</span>;
}

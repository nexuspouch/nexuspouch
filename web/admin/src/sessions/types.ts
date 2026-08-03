export interface SessionRow {
  uri: string;
  device: string;
  path?: string;
  agent?: string;
  project?: string;
  session_id?: string;
  kind?: string;
  title?: string;
  start_ms?: number;
  mtime?: number;
  duration_ms?: number;
  events?: number;
  size?: number;
}

export interface SessionTurn {
  role: string;
  content?: string;
  ts_ms?: number;
}

export interface SessionVersion {
  v: number;
  mtime: number;
  size: number;
}

export interface SessionDetail {
  uri: string;
  agent?: string;
  project?: string;
  session_id?: string;
  device?: string;
  kind?: string;
  size?: number;
  ref?: string;
  version?: number;
  protected?: boolean;
  truncated?: boolean;
  turns?: SessionTurn[];
  versions?: SessionVersion[];
}

export interface SearchHit {
  uri: string;
  path?: string;
  device?: string;
  snippet?: string;
  score_type?: string;
  occurrences?: number;
  occurrence_label?: string;
  fragment?: {
    turns?: Array<{ role?: string; content?: string; focus?: boolean }>;
  };
}

export interface BindingRow {
  id: string;
  external: string;
  space: string;
  folder: string;
  mode?: string;
}

export interface BindingReport {
  added?: number;
  updated?: number;
  errors?: string[];
}

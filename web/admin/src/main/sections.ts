export type SectionId =
  | 'overview'
  | 'devices'
  | 'storage'
  | 'import'
  | 'access'
  | 'catalog'
  | 'danger';

export type SectionDef = {
  id: SectionId;
  label: string;
};

export const SECTIONS: SectionDef[] = [
  { id: 'overview', label: '概览' },
  { id: 'devices', label: '设备与配对' },
  { id: 'storage', label: '存储' },
  { id: 'import', label: '换机导入' },
  { id: 'access', label: '身份与权限' },
  { id: 'catalog', label: '空间与版本' },
  { id: 'danger', label: '危险区' },
];

const IDS = new Set<string>(SECTIONS.map((s) => s.id));

export function parseSection(hash = location.hash): SectionId {
  const raw = (hash || '').replace(/^#\/?/, '').split(/[/?&]/)[0];
  if (IDS.has(raw)) return raw as SectionId;
  return 'overview';
}

export function sectionHash(id: SectionId): string {
  return `#${id}`;
}

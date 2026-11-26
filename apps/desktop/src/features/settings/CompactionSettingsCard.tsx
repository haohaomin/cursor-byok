import type { CompactionSettings } from "../../shared/api";
import { Button } from "../../shared/ui/Button";
import { TextInput } from "../../shared/ui/FormControls";
import { TitledCard } from "../../shared/ui/TitledCard";
import styles from "./CompactionSettingsCard.module.scss";

export function CompactionSettingsCard({
  settings,
  draft,
  editing,
  saving,
  onDraftChange,
  onEdit,
  onCancel,
  onSave,
}: {
  settings: CompactionSettings | null;
  draft: string;
  editing: boolean;
  saving: boolean;
  onDraftChange: (draft: string) => void;
  onEdit: () => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const action = editing ? (
    <div className={styles.actionGroup}>
      <Button size="small" disabled={saving} onClick={onCancel}>{t("取消")}</Button>
      <Button variant="primary" size="small" disabled={saving} onClick={onSave}>{saving ? t("保存中…") : t("保存")}</Button>
    </div>
  ) : (
    <button type="button" className={styles.headerAction} disabled={!settings} onClick={onEdit}>{t("编辑")}</button>
  );

  return <TitledCard title={t("自动压缩设置")} action={action}>
    <div className={styles.content}>
      {editing ? <div className={styles.row}>
        <div className={styles.description}>
          <strong>{t("自动压缩超时（秒）")}</strong>
          <small>{t("自动上下文压缩每批摘要调用的最长等待时间。上游响应较慢、压缩经常超时时可适当调大。")}</small>
        </div>
        <div className={styles.control}><TextInput
          value={draft}
          inputMode="numeric"
          aria-label={t("自动压缩超时（秒）")}
          onChange={(event) => onDraftChange(event.target.value.replace(/[^0-9]/g, ""))}
          onKeyDown={(event) => { if (event.key === "Enter") onSave(); }}
        /></div>
      </div> : <div className={styles.row}>
        <strong>{t("自动压缩超时（秒）")}</strong>
        <span className={styles.value}>{settings ? String(settings.auto_compaction_timeout_secs) : t("加载中…")}</span>
      </div>}
    </div>
  </TitledCard>;
}

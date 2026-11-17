// 职责：固定 Codex 统一供应商卡片，只提供成员勾选和自动模型预览。
// 调用方：ProviderList；依赖原供应商列表、统一成员 API、React Query。
// 红线：不编辑名称、凭证或映射；后端失败必须恢复勾选，启用动作复用原切换入口。
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Layers, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { providersApi } from "@/lib/api/providers";
import type { Provider } from "@/types";

export const CODEX_UNIFIED_PROVIDER_ID = "codex-unified";

export function isCodexUnifiedProvider(provider: Provider) {
  return (
    provider.id === CODEX_UNIFIED_PROVIDER_ID ||
    provider.meta?.providerType === "codex_unified"
  );
}

function storedMembers(provider?: Provider): string[] {
  const ids: unknown = provider?.settingsConfig.memberProviderIds;
  return Array.isArray(ids)
    ? ids.filter((id): id is string => typeof id === "string")
    : [];
}

interface Props {
  provider?: Provider;
  providers: Record<string, Provider>;
  isCurrent: boolean;
  onSwitch: (provider: Provider) => void;
}

export function CodexUnifiedProviderCard({
  provider,
  providers,
  isCurrent,
  onSwitch,
}: Props) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState(() => storedMembers(provider));
  const [expanded, setExpanded] = useState(false);
  const [failure, setFailure] = useState("");
  const [activating, setActivating] = useState(false);
  const candidates = Object.values(providers).filter(
    (item) =>
      !isCodexUnifiedProvider(item) &&
      item.category !== "official" &&
      item.meta?.providerType !== "codex_oauth",
  );
  const unavailableMembers = selected.filter(
    (id) => !candidates.some((item) => item.id === id),
  );
  const storedKey = JSON.stringify(storedMembers(provider));
  useEffect(() => {
    setSelected(JSON.parse(storedKey));
  }, [storedKey]);
  const preview = useQuery({
    queryKey: [
      "codexUnifiedPreview",
      selected,
      candidates.map((item) => [
        item.id,
        item.name,
        item.settingsConfig.modelCatalog,
        item.settingsConfig.model,
      ]),
    ],
    queryFn: () => providersApi.getCodexUnifiedPreview(selected),
    enabled: selected.length > 0,
  });
  const save = useMutation({
    mutationFn: (ids: string[]) => providersApi.saveCodexUnifiedMembers(ids),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["providers", "codex"] });
    },
  });
  const toggle = async (id: string, checked: boolean) => {
    const previous = selected;
    const next = checked
      ? [...selected, id]
      : selected.filter((member) => member !== id);
    setFailure("");
    setSelected(next);
    try {
      await save.mutateAsync(next);
    } catch (error) {
      setSelected(previous);
      setFailure(String(error));
    }
  };
  const activate = async () => {
    if (!provider) return;
    setActivating(true);
    try {
      await onSwitch(provider);
    } catch (error) {
      setFailure(String(error));
    } finally {
      setActivating(false);
    }
  };
  const rows = selected.length ? (preview.data ?? []) : [];
  return (
    <section
      data-testid="codex-unified-card"
      className={`rounded-xl border bg-card p-4 space-y-4 ${isCurrent ? "border-primary ring-1 ring-primary/20" : "border-border"}`}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3 min-w-0">
          <div className="h-10 w-10 rounded-xl bg-primary/10 text-primary grid place-items-center">
            <Layers className="h-5 w-5" />
          </div>
          <div>
            <h3 className="font-semibold">Codex</h3>
            <p className="text-xs text-muted-foreground">
              {t("provider.codexUnified.subtitle", "统一供应商")} ·{" "}
              {selected.length} {t("provider.codexUnified.sources", "个供应商")}{" "}
              · {rows.length} {t("provider.codexUnified.models", "个模型")}
            </p>
          </div>
        </div>
        <div className="flex gap-2 shrink-0">
          <Button
            variant="outline"
            size="sm"
            type="button"
            onClick={() => setExpanded(!expanded)}
            aria-expanded={expanded}
          >
            {t("provider.codexUnified.configure", "选择供应商")}
            <ChevronDown
              className={`ml-1 h-3 w-3 ${expanded ? "rotate-180" : ""}`}
            />
          </Button>
          <Button
            size="sm"
            type="button"
            onClick={activate}
            disabled={
              isCurrent ||
              save.isPending ||
              activating ||
              rows.length === 0 ||
              !!preview.error
            }
          >
            {activating && <Loader2 className="mr-1 h-3 w-3 animate-spin" />}
            {isCurrent
              ? t("provider.codexUnified.active", "使用中")
              : t("provider.codexUnified.activate", "使用 Codex")}
          </Button>
        </div>
      </div>
      {expanded && (
        <div className="grid gap-4 md:grid-cols-2 border-t border-border pt-4">
          <div className="space-y-2">
            <p className="text-xs font-medium text-muted-foreground">
              {t("provider.codexUnified.select", "勾选要一起使用的供应商")}
            </p>
            {candidates.length === 0 && (
              <p className="text-sm text-muted-foreground">
                {t(
                  "provider.codexUnified.empty",
                  "先添加一个真实供应商，即可在这里勾选。",
                )}
              </p>
            )}
            {candidates.map((item) => (
              <label
                key={item.id}
                className="flex cursor-pointer items-center gap-3 rounded-lg border border-border px-3 py-2"
              >
                <Checkbox
                  checked={selected.includes(item.id)}
                  disabled={save.isPending}
                  onCheckedChange={(checked) => void toggle(item.id, checked)}
                />
                <span className="min-w-0">
                  <span className="block text-sm truncate">{item.name}</span>
                  <span className="block text-[10px] text-muted-foreground">
                    {item.id.slice(0, 8)}
                  </span>
                </span>
              </label>
            ))}
            {unavailableMembers.map((id) => (
              <label
                key={id}
                className="flex items-center gap-3 rounded-lg border border-destructive/30 px-3 py-2 text-sm text-destructive"
              >
                <Checkbox
                  checked
                  disabled={save.isPending}
                  onCheckedChange={() => void toggle(id, false)}
                />
                <span>
                  {providers[id]?.name ?? id} ·{" "}
                  {t(
                    "provider.codexUnified.unavailable",
                    "来源已不可用，取消勾选可移除",
                  )}
                </span>
              </label>
            ))}
          </div>
          <div className="space-y-2 min-w-0">
            <p className="text-xs font-medium text-muted-foreground">
              {t("provider.codexUnified.preview", "Codex 中自动显示的模型")}
            </p>
            <div className="max-h-64 overflow-auto rounded-lg border border-border divide-y divide-border">
              {rows.map((row) => (
                <div key={row.alias} className="px-3 py-2">
                  <code className="block text-xs break-all">{row.alias}</code>
                  <span className="text-[10px] text-muted-foreground">
                    {row.providerName} → {row.actualModel}
                  </span>
                </div>
              ))}
              {rows.length === 0 && (
                <p className="p-3 text-xs text-muted-foreground">
                  {t(
                    "provider.codexUnified.previewEmpty",
                    "勾选后自动生成，不需要填写模型或映射。",
                  )}
                </p>
              )}
            </div>
          </div>
        </div>
      )}
      {(failure || preview.error) && (
        <p role="alert" className="text-xs text-destructive">
          {failure || String(preview.error)}
        </p>
      )}
    </section>
  );
}

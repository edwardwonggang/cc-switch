// 职责：Codex 多供应商模型路由开关；由 ProxyPanel 调用。
// 依赖：现有代理接管状态、Tauri 代理 API、React Query。
// 红线：后端确认成功后才更新开关；关闭必须可恢复原有单供应商行为。
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { proxyApi } from "@/lib/api/proxy";

const CODEX_MODEL_ROUTING_QUERY_KEY = ["codexModelRoutingEnabled"] as const;

export function CodexModelRoutingToggle({ takenOver }: { takenOver: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const {
    data: enabled = false,
    isLoading,
    error,
  } = useQuery({
    queryKey: CODEX_MODEL_ROUTING_QUERY_KEY,
    queryFn: () => proxyApi.getCodexModelRoutingEnabled(),
  });
  const update = useMutation({
    mutationFn: (value: boolean) => proxyApi.setCodexModelRoutingEnabled(value),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: CODEX_MODEL_ROUTING_QUERY_KEY,
      });
    },
    onError: (failure: Error) => toast.error(failure.message),
  });

  return (
    <div className="rounded-md border border-border bg-background/60 px-3 py-3 space-y-2">
      <div className="flex items-center justify-between gap-3">
        <Label htmlFor="codex-model-routing">
          {t("proxy.modelRouting.title", "Codex 多供应商模型路由")}
        </Label>
        <Switch
          id="codex-model-routing"
          checked={enabled}
          onCheckedChange={(value) => update.mutate(value)}
          disabled={isLoading || update.isPending || (!takenOver && !enabled)}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        {t(
          "proxy.modelRouting.hint",
          "按模型目录分发到全部供应商；同名模型按列表顺序优先。未知模型沿用默认供应商，关闭后恢复原策略。需要启用 Codex 接管。",
        )}
      </p>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {String(error)}
        </p>
      )}
    </div>
  );
}

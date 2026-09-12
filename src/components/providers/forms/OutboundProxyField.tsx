import { useTranslation } from "react-i18next";
import { FormLabel } from "@/components/ui/form";
import { Switch } from "@/components/ui/switch";

interface OutboundProxyFieldProps {
  /** Input id (used for the label's `htmlFor`); each form passes its own unique value. */
  id: string;
  /**
   * `true` = follow the global outbound proxy (default behaviour).
   * `false` = force a direct connection for this provider's upstream requests.
   */
  useGlobalProxy: boolean;
  /** Receives the raw switch state; the parent maps it to `meta.outboundProxy`. */
  onChange: (useGlobalProxy: boolean) => void;
}

/**
 * Provider-level outbound proxy switch (shared by the Claude and Codex forms).
 *
 * Why this component exists (Plan A):
 * cc-switch used to have a single, application-wide outbound proxy setting and
 * one shared HTTP client for every forwarded upstream request, so "use the
 * proxy" was all-or-nothing. On a corporate machine that forces a choice
 * between two broken states: an internal model endpoint (reachable only
 * directly) and an external vendor endpoint (reachable only through the
 * corporate proxy) could not both work.
 *
 * Turning this switch OFF persists `meta.outboundProxy = "direct"`, and the
 * Rust request forwarder then sends this provider's upstream requests straight
 * out — bypassing the configured global proxy *and* the OS/env system proxy.
 * Leaving it ON persists nothing, which is what keeps existing provider
 * configs, presets and deeplinks working unchanged.
 *
 * Deliberately two-state only (no "custom proxy URL"): the goal is to restore
 * the ability to opt out, not to reintroduce the larger v3.13 proxy config that
 * was removed in v3.14. See `OutboundProxyMode` in `@/types`.
 */
export function OutboundProxyField({
  id,
  useGlobalProxy,
  onChange,
}: OutboundProxyFieldProps) {
  const { t } = useTranslation();

  return (
    <div className="space-y-2">
      <FormLabel htmlFor={id}>
        {t("providerForm.outboundProxy", {
          defaultValue: "出站走代理",
        })}
      </FormLabel>
      <div className="flex items-center justify-between gap-4 rounded-lg border border-border-default px-3 py-2">
        <span className="text-xs text-muted-foreground">
          {useGlobalProxy
            ? t("providerForm.outboundProxyOn", {
                defaultValue: "使用全局出站代理（默认）",
              })
            : t("providerForm.outboundProxyOff", {
                defaultValue: "强制直连，绕过全局代理",
              })}
        </span>
        <Switch
          id={id}
          checked={useGlobalProxy}
          onCheckedChange={onChange}
          aria-label={t("providerForm.outboundProxy", {
            defaultValue: "出站走代理",
          })}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        {t("providerForm.outboundProxyHint", {
          defaultValue:
            "关闭后，该供应商的上游请求强制直连，绕过全局代理与系统代理；适用于公司内网模型等不应经过代理的地址。",
        })}
      </p>
    </div>
  );
}

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexUnifiedProviderCard } from "@/components/providers/CodexUnifiedProviderCard";
import { providersApi } from "@/lib/api/providers";
import type { Provider } from "@/types";

vi.mock("@/lib/api/providers", () => ({
  providersApi: {
    getCodexUnifiedPreview: vi.fn(),
    saveCodexUnifiedMembers: vi.fn(),
  },
}));

const unified = (ids: string[] = []): Provider => ({
  id: "codex-unified",
  name: "Codex",
  settingsConfig: { memberProviderIds: ids },
  meta: { providerType: "codex_unified" },
});
const a: Provider = {
  id: "a",
  name: "A",
  settingsConfig: { modelCatalog: { models: [{ model: "same-model" }] } },
};
const b: Provider = {
  id: "b",
  name: "B",
  settingsConfig: { modelCatalog: { models: [{ model: "same-model" }] } },
};

function mount(
  provider: Provider | undefined,
  isCurrent = false,
  onSwitch = vi.fn(),
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return {
    onSwitch,
    ...render(
      <QueryClientProvider client={queryClient}>
        <CodexUnifiedProviderCard
          provider={provider}
          providers={{
            a,
            b,
            "codex-unified": provider ?? unified(),
            official: {
              id: "official",
              name: "Official",
              category: "official",
              settingsConfig: {},
            },
          }}
          isCurrent={isCurrent}
          onSwitch={onSwitch}
        />
      </QueryClientProvider>,
    ),
  };
}

describe("内置 Codex 统一供应商", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(providersApi.getCodexUnifiedPreview).mockImplementation(
      async (ids) =>
        ids.map((id) => ({
          alias: `${id.toUpperCase()}-same-model`,
          providerId: id,
          providerName: id.toUpperCase(),
          actualModel: "same-model",
        })),
    );
    vi.mocked(providersApi.saveCodexUnifiedMembers).mockImplementation(
      async (ids) => unified(ids),
    );
  });

  it("没有成员时禁止启用，也不自动保存或切换普通供应商", () => {
    const { onSwitch } = mount(unified());
    expect(screen.getByRole("button", { name: /使用 Codex/ })).toBeDisabled();
    expect(providersApi.saveCodexUnifiedMembers).not.toHaveBeenCalled();
    expect(onSwitch).not.toHaveBeenCalled();
  });

  it("固定名称，无填写字段，只列可选成员并自动保存勾选", async () => {
    mount(undefined);
    fireEvent.click(screen.getByRole("button", { name: /选择供应商/ }));
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    fireEvent.click(screen.getAllByRole("checkbox")[0]);
    await waitFor(() =>
      expect(providersApi.saveCodexUnifiedMembers).toHaveBeenCalledWith(["a"]),
    );
    await waitFor(() =>
      expect(screen.getByText("A-same-model")).toBeInTheDocument(),
    );
  });

  it("同名真实模型显示两个来源别名，启用只切选固定卡", async () => {
    const { onSwitch } = mount(unified(["a", "b"]));
    fireEvent.click(screen.getByRole("button", { name: /选择供应商/ }));
    await waitFor(() =>
      expect(screen.getByText("A-same-model")).toBeInTheDocument(),
    );
    expect(screen.getByText("B-same-model")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /使用 Codex/ }));
    await waitFor(() =>
      expect(onSwitch).toHaveBeenCalledWith(unified(["a", "b"])),
    );
    expect(providersApi.saveCodexUnifiedMembers).not.toHaveBeenCalled();
  });

  it("当前卡取消最后成员失败时回滚勾选并显示错误", async () => {
    vi.mocked(providersApi.saveCodexUnifiedMembers).mockRejectedValue(
      new Error("请至少保留一个供应商"),
    );
    const { onSwitch } = mount(unified(["a"]), true);
    fireEvent.click(screen.getByRole("button", { name: /选择供应商/ }));
    const checkbox = screen.getAllByRole("checkbox")[0];
    expect(checkbox).toBeChecked();
    fireEvent.click(checkbox);
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "请至少保留一个供应商",
      ),
    );
    expect(checkbox).toBeChecked();
    expect(onSwitch).not.toHaveBeenCalled();
  });

  it("保存被拒绝时保留原成员，不借机切换到其它来源", async () => {
    vi.mocked(providersApi.saveCodexUnifiedMembers).mockRejectedValue(
      new Error("供应商已不存在"),
    );
    const { onSwitch } = mount(unified(["a"]));
    fireEvent.click(screen.getByRole("button", { name: /选择供应商/ }));
    fireEvent.click(screen.getAllByRole("checkbox")[1]);
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("供应商已不存在"),
    );
    expect(screen.getAllByRole("checkbox")[0]).toBeChecked();
    expect(screen.getAllByRole("checkbox")[1]).not.toBeChecked();
    expect(onSwitch).not.toHaveBeenCalled();
  });
});

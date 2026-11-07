import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CodexModelRoutingToggle } from "@/components/proxy/CodexModelRoutingToggle";
import { proxyApi } from "@/lib/api/proxy";

vi.mock("@/lib/api/proxy", () => ({
  proxyApi: {
    getCodexModelRoutingEnabled: vi.fn(),
    setCodexModelRoutingEnabled: vi.fn(),
  },
}));

function mount(takenOver: boolean) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <CodexModelRoutingToggle takenOver={takenOver} />
    </QueryClientProvider>,
  );
}

describe("Codex 多供应商模型路由开关", () => {
  beforeEach(() => vi.clearAllMocks());

  it("未接管时禁止首次开启，已有开启状态仍可关闭", async () => {
    vi.mocked(proxyApi.getCodexModelRoutingEnabled).mockResolvedValue(false);
    const view = mount(false);
    const toggle = screen.getByRole("switch");
    await waitFor(() => expect(toggle).toBeDisabled());
    view.unmount();
    vi.mocked(proxyApi.getCodexModelRoutingEnabled).mockResolvedValue(true);
    mount(false);
    await waitFor(() => expect(screen.getByRole("switch")).toBeEnabled());
    expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "true");
  });

  it("后端保存并读回后才更新勾选状态", async () => {
    let enabled = false;
    vi.mocked(proxyApi.getCodexModelRoutingEnabled).mockImplementation(
      async () => enabled,
    );
    vi.mocked(proxyApi.setCodexModelRoutingEnabled).mockImplementation(
      async (value) => {
        enabled = value;
      },
    );
    mount(true);
    await waitFor(() => expect(screen.getByRole("switch")).toBeEnabled());
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() =>
      expect(proxyApi.setCodexModelRoutingEnabled).toHaveBeenCalledWith(true),
    );
    await waitFor(() =>
      expect(screen.getByRole("switch")).toHaveAttribute(
        "aria-checked",
        "true",
      ),
    );
  });

  it("保存失败保留后端原状态", async () => {
    vi.mocked(proxyApi.getCodexModelRoutingEnabled).mockResolvedValue(false);
    vi.mocked(proxyApi.setCodexModelRoutingEnabled).mockRejectedValue(
      new Error("fake failure"),
    );
    mount(true);
    await waitFor(() => expect(screen.getByRole("switch")).toBeEnabled());
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() =>
      expect(proxyApi.setCodexModelRoutingEnabled).toHaveBeenCalledWith(true),
    );
    await waitFor(() => expect(screen.getByRole("switch")).toBeEnabled());
    expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "false");
  });
});

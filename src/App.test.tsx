import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Collection, Progress } from "./types";

const mocks = vi.hoisted(() => ({
  start: vi.fn(),
  items: vi.fn(),
  catalog: vi.fn(),
}));
vi.mock("./bridge", () => {
  const make = (ownerId: number): Collection[] => [
    {
      id: `${ownerId}:photos:1`,
      ownerId,
      kind: "photos",
      title: ownerId < 0 ? "Альбом группы" : "Лето",
      count: 4,
      cover: null,
      description: "",
      available: true,
      error: null,
    },
  ];
  return {
    native: false,
    demo: true,
    backend: {
      connect: async () => ({ id: 123, name: "Демо", avatar: null }),
      sources: async () => ({
        items: [
          { id: 123, name: "Демо", community: false, avatar: null },
          { id: -77, name: "Группа", community: true, avatar: null },
        ],
        warning: null,
      }),
      catalog: mocks.catalog.mockImplementation(async (id: number) => make(id)),
      items: mocks.items.mockImplementation(
        async (_c: Collection, offset: number) => ({
          items: [offset, offset + 1].map((i) => ({
            id: `item-${i}`,
            title: `Фото ${i}`,
            thumbnail: null,
            date: null,
            duration: null,
            downloadable: true,
            reason: null,
          })),
          total: 4,
          nextOffset: offset === 0 ? 2 : null,
        }),
      ),
      tools: async () => false,
      progress: async () => () => {},
      folder: async () => "/test/backup",
      start: mocks.start.mockImplementation(
        async () =>
          ({
            id: "test-job",
            status: "completed",
            phase: "finished",
            total: 4,
            completed: 4,
            skipped: 0,
            failed: 0,
            unavailable: 0,
            bytes: 4000,
            current: "",
            currentBytes: 0,
            currentTotal: null,
            speed: 0,
            folder: "/test",
            errors: [],
            startedAt: 0,
          }) satisfies Progress,
      ),
      status: async () => null,
      control: async () => {},
      openFolder: async () => {},
      logout: async () => {},
    },
  };
});
import App from "./App";

describe("backup selection", () => {
  it("excludes one photo from all pages and keeps a second community in the same job", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(
      await screen.findByRole("button", { name: "Просмотреть Лето" }),
    );
    const dialog = screen.getByRole("dialog");
    await user.click(
      within(dialog).getByRole("button", { name: "Выбрать весь альбом" }),
    );
    const first = await within(dialog).findByRole("checkbox", {
      name: "Выбрать Фото 0",
    });
    await user.click(first);
    expect(first).not.toBeChecked();
    await user.click(
      within(dialog).getByRole("button", { name: /Показать ещё/ }),
    );
    expect(
      await within(dialog).findByRole("checkbox", { name: "Выбрать Фото 2" }),
    ).toBeChecked();
    await user.click(within(dialog).getByRole("button", { name: /Готово/ }));
    expect(
      screen.getByText("3 выбрано", { selector: ".download-summary strong" }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Группа/ }));
    await user.click(
      await screen.findByRole("checkbox", { name: "Выбрать Альбом группы" }),
    );
    expect(screen.getByText("7 выбрано")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /СОХРАНИТЬ В/ }));
    await user.click(screen.getByRole("button", { name: "Запустить демо" }));
    await waitFor(() =>
      expect(mocks.start).toHaveBeenCalledWith(
        "/test/backup",
        expect.arrayContaining([
          {
            collectionId: "123:photos:1",
            itemIds: null,
            excludedIds: ["item-0"],
          },
          { collectionId: "-77:photos:1", itemIds: null, excludedIds: [] },
        ]),
        false,
      ),
    );
    expect(
      await screen.findByText("Демонстрация завершена"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Это симуляция. Файлы на диск не записывались."),
    ).toBeInTheDocument();
  });
  it("selects an individual file without selecting the rest of an unloaded album", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(
      await screen.findByRole("button", { name: "Просмотреть Лето" }),
    );
    const dialog = screen.getByRole("dialog");
    await user.click(
      await within(dialog).findByRole("checkbox", { name: "Выбрать Фото 0" }),
    );
    expect(
      within(dialog).getByRole("checkbox", { name: "Выбрать Фото 1" }),
    ).not.toBeChecked();
    await user.click(
      within(dialog).getByRole("button", { name: /Показать ещё/ }),
    );
    expect(
      await within(dialog).findByRole("checkbox", { name: "Выбрать Фото 2" }),
    ).not.toBeChecked();
    await user.click(within(dialog).getByRole("button", { name: /Готово/ }));
    expect(
      screen.getByText("1 выбрано", { selector: ".download-summary strong" }),
    ).toBeInTheDocument();
  });
});

import { describe, expect, it } from "vitest";
import {
  acceptSourceStart,
  changeSourceDraft,
  createSourceEditingState,
  failSourceRequest,
  isCurrentSourceRequest,
  isSourceDirty,
  isWaitingForSourceStart,
  requestSourceTranslation,
} from "./sourceEditing";

describe("原文编辑与失焦重译", () => {
  it("可以从空面板输入，重复失焦只提交一次，保留中文和换行", () => {
    const edited = changeSourceDraft(createSourceEditingState(), "第一行\n第二行 🦀");
    const submitted = requestSourceTranslation(edited);
    const repeated = requestSourceTranslation(submitted.state);

    expect(submitted.request?.text).toBe("第一行\n第二行 🦀");
    expect(repeated.request).toBeNull();
    expect(isWaitingForSourceStart(repeated.state)).toBe(true);
  });

  it("选中或复制原文后失焦，文字没改变就不重新翻译", () => {
    const translated = acceptSourceStart(createSourceEditingState(), "原文")!.state;

    expect(requestSourceTranslation(translated).request).toBeNull();
    expect(isSourceDirty(translated)).toBe(false);
  });

  it("提交后继续输入，迟到的开始事件保留新草稿", () => {
    const first = requestSourceTranslation(changeSourceDraft(createSourceEditingState(), "第一版"));
    const secondDraft = changeSourceDraft(first.state, "第二版");
    const started = acceptSourceStart(secondDraft, "第一版", first.request!.id)!;

    expect(started.state.draft).toBe("第二版");
    expect(started.state.source).toBe("第一版");
    expect(isSourceDirty(started.state)).toBe(true);
    expect(started.resetEditor).toBe(false);
    expect(requestSourceTranslation(started.state).request?.text).toBe("第二版");
  });

  it("连续提交两个版本，只接受最新版本的开始事件", () => {
    const first = requestSourceTranslation(changeSourceDraft(createSourceEditingState(), "第一版"));
    const second = requestSourceTranslation(changeSourceDraft(first.state, "第二版"));

    expect(acceptSourceStart(second.state, "第一版", first.request!.id)).toBeNull();
    const started = acceptSourceStart(second.state, "第二版", second.request!.id)!;
    expect(started.state.source).toBe("第二版");
    expect(started.state.draft).toBe("第二版");
    expect(isWaitingForSourceStart(started.state)).toBe(false);
  });

  it("提交新版本后改回旧原文，仍会撤销待翻译的新版本", () => {
    const original = acceptSourceStart(createSourceEditingState(), "原文")!.state;
    const changed = requestSourceTranslation(changeSourceDraft(original, "修改版"));
    const reverted = requestSourceTranslation(changeSourceDraft(changed.state, "原文"));

    expect(reverted.request?.text).toBe("原文");
    expect(isCurrentSourceRequest(reverted.state, changed.request!)).toBe(false);
  });

  it.each(["", " \n\t "])("清空原文 %j 会提交清空，不调用空文本翻译", (empty) => {
    const original = acceptSourceStart(createSourceEditingState(), "原文")!.state;
    const cleared = requestSourceTranslation(changeSourceDraft(original, empty));
    const started = acceptSourceStart(cleared.state, "", cleared.request!.id)!;

    expect(cleared.request?.text).toBe("");
    expect(started.state.draft).toBe("");
    expect(started.resetEditor).toBe(empty !== "");
    expect(isSourceDirty(started.state)).toBe(false);
    expect(requestSourceTranslation(started.state).request).toBeNull();
  });

  it("新的划词或历史原文取消旧的待提交请求", () => {
    const pending = requestSourceTranslation(changeSourceDraft(createSourceEditingState(), "手动输入"));
    const external = acceptSourceStart(pending.state, "新的划词")!;

    expect(external.state.draft).toBe("新的划词");
    expect(external.resetEditor).toBe(true);
    expect(isCurrentSourceRequest(external.state, pending.request!)).toBe(false);
    expect(acceptSourceStart(external.state, "手动输入", pending.request!.id)).toBeNull();
  });

  it("提交失败保留草稿，再次失焦可以重试相同文本", () => {
    const submitted = requestSourceTranslation(changeSourceDraft(createSourceEditingState(), "待翻译原文"));
    const failed = failSourceRequest(submitted.state, submitted.request!)!;
    const retry = requestSourceTranslation(failed);

    expect(failed.draft).toBe("待翻译原文");
    expect(retry.request?.text).toBe("待翻译原文");
    expect(retry.request!.id).toBeGreaterThan(submitted.request!.id);
  });

  it("翻译请求失败后可以重试已开始的同一原文", () => {
    const submitted = requestSourceTranslation(changeSourceDraft(createSourceEditingState(), "原文"));
    const started = acceptSourceStart(submitted.state, "原文", submitted.request!.id)!.state;
    const retry = requestSourceTranslation(started, true);

    expect(retry.request?.text).toBe("原文");
    expect(requestSourceTranslation(retry.state, true).request).toBeNull();
  });
});

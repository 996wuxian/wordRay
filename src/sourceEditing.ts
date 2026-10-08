export interface SourceTranslationRequest {
  readonly id: number;
  readonly text: string;
  readonly revision: number;
}

export interface SourceEditingState {
  readonly draft: string;
  readonly source: string;
  readonly target: string;
  readonly revision: number;
  readonly sequence: number;
  readonly request: (SourceTranslationRequest & { readonly started: boolean }) | null;
}

const translationSource = (text: string): string => text.trim() ? text : "";

export function createSourceEditingState(): SourceEditingState {
  return { draft: "", source: "", target: "", revision: 0, sequence: 0, request: null };
}

export function changeSourceDraft(state: SourceEditingState, draft: string): SourceEditingState {
  return draft === state.draft ? state : { ...state, draft, revision: state.revision + 1 };
}

export function isSourceDirty(state: SourceEditingState): boolean {
  return translationSource(state.draft) !== state.source;
}

export function isWaitingForSourceStart(state: SourceEditingState): boolean {
  return state.request !== null && !state.request.started;
}

export function isCurrentSourceRequest(
  state: SourceEditingState,
  request: SourceTranslationRequest,
): boolean {
  return state.request?.id === request.id;
}

/** 元素失焦与窗口失焦可能连续发生，同一份待提交原文只发起一次请求。 */
export function requestSourceTranslation(state: SourceEditingState, retry = false): {
  state: SourceEditingState;
  request: SourceTranslationRequest | null;
} {
  const text = translationSource(state.draft);
  if ((isWaitingForSourceStart(state) && state.request?.text === text) ||
      (!retry && text === state.target)) return { state, request: null };
  const request = { id: state.sequence + 1, text, revision: state.revision };
  return {
    state: { ...state, target: text, sequence: request.id, request: { ...request, started: false } },
    request,
  };
}

/** 手动翻译开始时保留随后输入的草稿；新的划词或历史记录则替换当前原文。 */
export function acceptSourceStart(
  state: SourceEditingState,
  source: string,
  requestId?: number,
): { state: SourceEditingState; resetEditor: boolean } | null {
  if (requestId === undefined) {
    return {
      state: {
        ...state, source, draft: source, target: source,
        revision: state.revision + 1, request: null,
      },
      resetEditor: true,
    };
  }
  const request = state.request;
  if (!request || request.id !== requestId || request.text !== source || request.started) return null;
  return {
    state: {
      ...state, source,
      draft: state.revision === request.revision ? source : state.draft,
      request: { ...request, started: true },
    },
    resetEditor: state.revision === request.revision && state.draft !== source,
  };
}

export function failSourceRequest(
  state: SourceEditingState,
  request: SourceTranslationRequest,
): SourceEditingState | null {
  return isCurrentSourceRequest(state, request)
    ? { ...state, target: state.source, request: null }
    : null;
}

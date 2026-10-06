/**
 * 只测纯逻辑（不碰 DOM）。
 *
 * 这里的偏移换算最容易出错，而界面上算错一位很难用肉眼发现，
 * 所以用测试钉住：重复字词的单调前进、定位失败的跳过、区间切片的边界。
 */

import { describe, expect, it } from "vitest";
import {
  buildAlignment,
  mapAlignedSelection,
  overlaps,
  sliceByRanges,
} from "./selectionAlign";
import type { AlignedPair, Span } from "./selectionAlign";

describe("buildAlignment", () => {
  it("把片段定位成区间", () => {
    const pairs = buildAlignment("你好世界", "Hello world", [
      { src: "你好", dst: "Hello" },
      { src: "世界", dst: "world" },
    ]);
    expect(pairs).toEqual([
      { src: [0, 2], dst: [0, 5] },
      { src: [2, 4], dst: [6, 11] },
    ]);
  });

  it("重复出现的字词要单调前进，不能都指向第一次", () => {
    const pairs = buildAlignment("ab ab", "x x", [
      { src: "ab", dst: "x" },
      { src: "ab", dst: "x" },
    ]);
    expect(pairs).toEqual([
      { src: [0, 2], dst: [0, 1] },
      { src: [3, 5], dst: [2, 3] },
    ]);
  });

  it("英文语序不同时，dst 允许回退到全局查找", () => {
    // 「以后」在中文中间，对应的 "later" 在英文句尾；「再说」反而对应更靠前的 "talk"。
    // 如果 dst 也强制单调前进，最后一对会因为"位置更靠前"而整对丢失。
    const pairs = buildAlignment("我们以后再说", "Let's talk later", [
      { src: "我们", dst: "Let's" },
      { src: "以后", dst: "later" },
      { src: "再说", dst: "talk" },
    ]);
    expect(pairs).toEqual([
      { src: [0, 2], dst: [0, 5] },
      { src: [2, 4], dst: [11, 16] },
      { src: [4, 6], dst: [6, 10] },
    ]);
  });

  it("定位不到的片段直接跳过，不猜", () => {
    const pairs = buildAlignment("你好世界", "Hello world", [
      { src: "你好", dst: "Hello" },
      { src: "不存在的字", dst: "nope" },
      { src: "世界", dst: "world" },
    ]);
    expect(pairs).toEqual([
      { src: [0, 2], dst: [0, 5] },
      { src: [2, 4], dst: [6, 11] },
    ]);
  });

  it("空片段与空数组都是安全输入", () => {
    expect(buildAlignment("你好", "hi", [])).toEqual([]);
    expect(buildAlignment("你好", "hi", [{ src: "", dst: "" }])).toEqual([]);
  });
});

describe("overlaps", () => {
  it("相交为真", () => {
    expect(overlaps([2, 6], 5, 9)).toBe(true);
    expect(overlaps([2, 6], 0, 3)).toBe(true);
  });

  it("只接触端点不算相交（半开区间）", () => {
    expect(overlaps([2, 6], 6, 9)).toBe(false);
    expect(overlaps([2, 6], 0, 2)).toBe(false);
  });
});

describe("mapAlignedSelection", () => {
  const source = "还是上词对齐模型。";
  const result = "or move to a word alignment model.";
  const modelSource: Span = [source.indexOf("模型"), source.indexOf("模型") + 2];
  const modelTarget: Span = [result.indexOf("model"), result.indexOf("model") + 5];

  it("截图中只选模型时不能按词组的字符比例猜出 ment model", () => {
    const alignment = buildAlignment(source, result, [{ src: "词对齐模型", dst: "word alignment model" }]);
    expect(mapAlignedSelection(source, result, alignment, "src", modelSource)).toBeNull();
  });

  it("已有完整词项时，模型准确映射到整个 model", () => {
    const alignment = buildAlignment(source, result, [{ src: "模型", dst: "model" }]);
    expect(mapAlignedSelection(source, result, alignment, "src", modelSource)).toEqual([modelTarget]);
  });

  it("英文反向选择 model 映射到模型", () => {
    const alignment = buildAlignment(source, result, [{ src: "模型", dst: "model" }]);
    expect(mapAlignedSelection(source, result, alignment, "dst", modelTarget)).toEqual([modelSource]);
  });

  it("英文反向选择词组的一部分时需要进一步语义对齐", () => {
    const alignment = buildAlignment(source, result, [{ src: "词对齐模型", dst: "word alignment model" }]);
    expect(mapAlignedSelection(source, result, alignment, "dst", modelTarget)).toBeNull();
  });

  it("完整选择词组直接映射整个目标词组", () => {
    const alignment = buildAlignment(source, result, [{ src: "词对齐模型", dst: "word alignment model" }]);
    const phrase: Span = [source.indexOf("词对齐模型"), source.indexOf("词对齐模型") + 5];
    expect(mapAlignedSelection(source, result, alignment, "src", phrase)).toEqual([alignment[0].dst]);
  });

  it("选中的标点和空白缺口不妨碍词项覆盖", () => {
    const text = "你好， 世界！";
    const translation = "Hello, world!";
    const alignment = buildAlignment(text, translation, [
      { src: "你好", dst: "Hello" },
      { src: "世界", dst: "world" },
    ]);
    expect(mapAlignedSelection(text, translation, alignment, "src", [0, text.length])).toEqual([
      [0, 5],
      [7, 12],
    ]);
  });

  it("英文反向选区中的标点和空白同样不要求有词项", () => {
    const alignment = buildAlignment("你好，世界！", "Hello, world!", [
      { src: "你好", dst: "Hello" },
      { src: "世界", dst: "world" },
    ]);
    expect(mapAlignedSelection("你好，世界！", "Hello, world!", alignment, "dst", [0, 13])).toEqual([
      [0, 2],
      [3, 5],
    ]);
  });

  it("重复词的定位顺序不能代替模型对出现位置的确认", () => {
    const alignment = buildAlignment("模型 模型", "model model", [
      { src: "模型", dst: "model" },
      { src: "模型", dst: "model" },
    ]);
    expect(mapAlignedSelection("模型 模型", "model model", alignment, "src", [3, 5])).toBeNull();
  });

  it("译文语序回退遇到重复词时，需要语义请求消歧", () => {
    const text = "甲以后乙";
    const translation = "first model, second model later";
    const alignment = buildAlignment(text, translation, [
      { src: "甲", dst: "model" },
      { src: "以后", dst: "later" },
      { src: "乙", dst: "model" },
    ]);
    expect(alignment).toEqual([
      { src: [0, 1], dst: [6, 11] },
      { src: [1, 3], dst: [26, 31] },
      { src: [3, 4], dst: [6, 11] },
    ]);
    expect(mapAlignedSelection(text, translation, alignment, "src", [3, 4])).toBeNull();
  });

  it("反向选中重复译文时，不能返回共享该位置的多个原文词", () => {
    const text = "甲以后乙";
    const translation = "first model, second model later";
    const alignment = buildAlignment(text, translation, [
      { src: "甲", dst: "model" },
      { src: "以后", dst: "later" },
      { src: "乙", dst: "model" },
    ]);
    expect(mapAlignedSelection(text, translation, alignment, "dst", [6, 11])).toBeNull();
  });

  it("反向映射的原文词重复出现时，也交给语义请求确认", () => {
    const alignment = buildAlignment("模型 模型", "first second", [
      { src: "模型", dst: "first" },
      { src: "模型", dst: "second" },
    ]);
    expect(mapAlignedSelection("模型 模型", "first second", alignment, "dst", [6, 12])).toBeNull();
  });

  it("选中范围存在真正漏词时不能返回不完整高亮", () => {
    const alignment = buildAlignment("你好美丽世界", "Hello beautiful world", [
      { src: "你好", dst: "Hello" },
      { src: "世界", dst: "world" },
    ]);
    expect(mapAlignedSelection("你好美丽世界", "Hello beautiful world", alignment, "src", [0, 6])).toBeNull();
  });

  it("英文选区中的空格可以跳过，但漏掉的英文词不能跳过", () => {
    const alignment = buildAlignment("你好世界", "Hello beautiful world", [
      { src: "你好", dst: "Hello" },
      { src: "世界", dst: "world" },
    ]);
    expect(mapAlignedSelection("你好世界", "Hello beautiful world", alignment, "dst", [0, 21])).toBeNull();
  });

  it.each([
    ["alignment model", "ment"],
    ["model", "mod"],
    ["don't", "don"],
    ["don't", "t"],
  ])("目标 %s 中的 %s 不是完整英文单词，不能高亮", (translation, fragment) => {
    const alignment = buildAlignment("模型", translation, [{ src: "模型", dst: fragment }]);
    expect(mapAlignedSelection("模型", translation, alignment, "src", [0, 2])).toBeNull();
  });

  it("不把英文原文中的子词片段当作反向词项", () => {
    const alignment = buildAlignment("模型", "alignment model", [{ src: "模型", dst: "ment" }]);
    expect(mapAlignedSelection("模型", "alignment model", alignment, "dst", [5, 9])).toBeNull();
  });

  it("目标语序变化时排序区间，并保留未对应的目标间隙", () => {
    const text = "我们以后再说";
    const translation = "Let's talk about it later";
    const alignment = buildAlignment(text, translation, [
      { src: "我们", dst: "Let's" },
      { src: "以后", dst: "later" },
      { src: "再说", dst: "talk" },
    ]);
    expect(mapAlignedSelection(text, translation, alignment, "src", [2, 6])).toEqual([
      [6, 10],
      [20, 25],
    ]);
  });

  it("重叠目标只合并实际覆盖的范围，不修改对齐数据", () => {
    const alignment: AlignedPair[] = [
      { src: [0, 1], dst: [0, 10] },
      { src: [1, 2], dst: [6, 16] },
    ];
    expect(mapAlignedSelection("你好", "Hello wide world", alignment, "src", [0, 2])).toEqual([[0, 16]]);
    expect(alignment[0].dst).toEqual([0, 10]);
  });

  it("emoji 前后的偏移始终按 UTF-16 码元计算", () => {
    const text = "😀模型";
    const translation = "😀 model";
    const alignment = buildAlignment(text, translation, [{ src: "模型", dst: "model" }]);
    expect(alignment).toEqual([{ src: [2, 4], dst: [3, 8] }]);
    expect(mapAlignedSelection(text, translation, alignment, "src", [2, 4])).toEqual([[3, 8]]);
    expect(mapAlignedSelection(text, translation, alignment, "dst", [3, 8])).toEqual([[2, 4]]);
  });

  it("不能忽略选中范围中没有对齐的 emoji", () => {
    const alignment = buildAlignment("😀模型", "😀 model", [{ src: "模型", dst: "model" }]);
    expect(mapAlignedSelection("😀模型", "😀 model", alignment, "src", [0, 4])).toBeNull();
  });

  it.each<Span>([[0, 0], [-1, 2], [0, 3], [0.5, 2]])("非法选区 %j 不能用于映射", (start, end) => {
    expect(mapAlignedSelection("模型", "model", [{ src: [0, 2], dst: [0, 5] }], "src", [start, end])).toBeNull();
  });

  it("不能高亮越界或切断代理对的已定位区间", () => {
    const alignment: AlignedPair[] = [
      { src: [0, 2], dst: [0, 6] },
      { src: [0, 2], dst: [1, 2] },
    ];
    expect(mapAlignedSelection("模型", "😀", alignment, "src", [0, 2])).toBeNull();
    expect(mapAlignedSelection("😀模型", "model", [{ src: [1, 4], dst: [0, 5] }], "src", [1, 4])).toBeNull();
  });

  it("非法区间不会混入合法词项的高亮结果", () => {
    const alignment: AlignedPair[] = [
      { src: [0, 2], dst: [0, 6] },
      { src: [0, 2], dst: [0, 5] },
    ];
    expect(mapAlignedSelection("模型", "model", alignment, "src", [0, 2])).toEqual([[0, 5]]);
  });

  it("没有对齐数据时需要语义请求，不按全文字符比例猜测", () => {
    expect(mapAlignedSelection("模型", "word alignment model", [], "src", [0, 2])).toBeNull();
  });
});

describe("sliceByRanges", () => {
  it("没有高亮时就是整段", () => {
    expect(sliceByRanges("hello", [])).toEqual([{ text: "hello", hit: false }]);
  });

  it("中间一段高亮切成三片", () => {
    expect(sliceByRanges("abcdef", [[2, 4]])).toEqual([
      { text: "ab", hit: false },
      { text: "cd", hit: true },
      { text: "ef", hit: false },
    ]);
  });

  it("从头到尾覆盖时只有一片", () => {
    expect(sliceByRanges("abc", [[0, 3]])).toEqual([{ text: "abc", hit: true }]);
  });

  it("边界值为 0 或长度时不会产生空片", () => {
    const slices = sliceByRanges("abc", [[0, 1]]);
    expect(slices).toEqual([
      { text: "a", hit: true },
      { text: "bc", hit: false },
    ]);
    expect(slices.every((slice) => slice.text.length > 0)).toBe(true);
  });

  it("多个区间合并后仍是连续覆盖，不丢字", () => {
    const slices = sliceByRanges("abcdef", [
      [1, 2],
      [4, 5],
    ]);
    expect(slices.map((slice) => slice.text).join("")).toBe("abcdef");
    expect(slices.filter((slice) => slice.hit).map((slice) => slice.text)).toEqual(["b", "e"]);
  });
});

//! 标签组结果胶囊：收件箱列表与读信页共用。
//!
//! 选择题/是否题胶囊显示标签名与标签颜色；打分题胶囊只显示数字，
//! 按数值在组区间里的相对位置分 5 档取色（1 灰、2 蓝、3 黄、4 橙、5 红）。
//! 传了 onClick 就渲染成按钮，否则只读展示，交给调用方决定。

import type { GroupResult } from "./api";

/**
 * 打分胶囊的颜色档：把数值在组区间里的相对位置切成 5 档，最低 1、最高 5。
 *
 * 上下限正好是 1–5 时，1 灰、2 蓝、3 黄、4 橙、5 红一一对应。
 */
export function scoreBand(value: number, min: number, max: number): number {
  if (max <= min) return 1;
  const ratio = (value - min) / (max - min);
  const clamped = Math.min(1, Math.max(0, ratio));
  return Math.round(1 + clamped * 4);
}

/** 5 档颜色对应的固定色板键：1 灰、2 蓝、3 黄、4 橙、5 红。 */
const SCORE_BAND_COLORS = ["gray", "blue", "amber", "orange", "red"] as const;

export function scoreBandColorKey(band: number): string {
  const index = Math.min(SCORE_BAND_COLORS.length, Math.max(1, Math.round(band))) - 1;
  return SCORE_BAND_COLORS[index];
}

/** 打分题的可选数字：下限到上限逐个列出来；缺上下限时按默认 1–5。 */
export function scoreOptions(min: number | null, max: number | null): number[] {
  const low = min ?? 1;
  const high = max ?? 5;
  const out: number[] = [];
  for (let value = low; value <= high; value += 1) out.push(value);
  return out;
}

/** 组结果的胶囊文字：选择题/是否题是标签名，打分题是数字。 */
export function pillText(result: GroupResult): string {
  if (result.kind === "score") {
    return result.scoreValue === null ? "" : String(result.scoreValue);
  }
  return result.labelName ?? "";
}

/** 打分题的胶囊档位；其它题型返回 null。 */
export function pillScoreBand(result: GroupResult): number | null {
  if (result.kind !== "score" || result.scoreValue === null) return null;
  return scoreBand(
    result.scoreValue,
    result.scoreMin ?? result.scoreValue,
    result.scoreMax ?? result.scoreValue,
  );
}

/** 一个组结果的胶囊；传了 onClick 才是可点的按钮。 */
export function GroupResultPill({
  result,
  onClick,
}: {
  result: GroupResult;
  onClick?: (result: GroupResult) => void;
}) {
  const text = pillText(result);
  if (text === "") return null;
  const band = pillScoreBand(result);
  const colorKey = band !== null ? scoreBandColorKey(band) : (result.labelColor ?? "gray");
  const className = band !== null ? "inbox-row-label inbox-row-score" : "inbox-row-label";
  const bandAttr = band !== null ? { "data-score-band": band } : {};
  if (onClick) {
    return (
      <button
        type="button"
        className={className}
        data-label-color={colorKey}
        {...bandAttr}
        onClick={(event) => {
          event.stopPropagation();
          onClick(result);
        }}
      >
        {text}
      </button>
    );
  }
  return (
    <span className={className} data-label-color={colorKey} {...bandAttr}>
      {text}
    </span>
  );
}

/** 一行要显示的多个组胶囊；没结果的组不占位。 */
export function GroupPills({
  results,
  onClick,
}: {
  results: GroupResult[];
  onClick?: (result: GroupResult) => void;
}) {
  return (
    <>
      {results.map((result) => (
        <GroupResultPill key={result.groupId} result={result} onClick={onClick} />
      ))}
    </>
  );
}
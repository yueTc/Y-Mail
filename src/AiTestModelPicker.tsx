import { useId, useState } from "react";

import { t } from "./i18n";

interface Props {
  value: string;
  models: string[];
  defaultModel: string;
  disabled: boolean;
  onChange: (model: string) => void;
}

/** 测试模型可从站点列表选择，也可填写中转站自己的模型编号。 */
export default function AiTestModelPicker({
  value,
  models,
  defaultModel,
  disabled,
  onChange,
}: Props) {
  const id = useId();
  const [manual, setManual] = useState(false);
  const choices = [...new Set([defaultModel, ...models].map((model) => model.trim()).filter(Boolean))];
  const manualEntry = manual || !choices.includes(value);

  return (
    <div className="ai-test-model">
      <label htmlFor={id}>{t("测试模型")}</label>
      <select
        id={id}
        value={manualEntry ? "" : value}
        disabled={disabled}
        onChange={(event) => {
          setManual(event.target.value === "");
          onChange(event.target.value);
        }}
      >
        {choices.map((model) => <option key={model} value={model}>{model}</option>)}
        <option value="">{t("手工填写模型编号")}</option>
      </select>
      {manualEntry && (
        <input
          aria-label={t("手填测试模型")}
          value={value}
          placeholder={t("填写中转站支持的模型编号")}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
        />
      )}
    </div>
  );
}

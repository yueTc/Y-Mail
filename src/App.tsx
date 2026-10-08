import { useCallback, useEffect, useState } from "react";

import { api, describeError, type AppSettings, type Contact } from "./api";
import ContactsWorkspace from "./ContactsWorkspace";
import FirstRunWizard from "./FirstRunWizard";
import InboxPanel, { type InboxComposeSeed } from "./InboxPanel";
import SettingsWorkspace from "./SettingsWorkspace";
import WorkspaceShell, { type WorkspaceMode } from "./WorkspaceShell";
import { t, useLanguage } from "./i18n";

/**
 * 顶层只做五件事：
 * 1. 记住当前是收件箱、通讯录还是设置；
 * 2. 记住代理列表改过几次，通知账号面板重拉「指定代理」可选项；
 * 3. 把通讯录点「写邮件」的结果转成收件箱的一次写信请求；
 * 4. 第一次启动时弹「邮件数据放哪」向导；
 * 5. 把三块内容交给工作区外壳，切换时三边状态都保留。
 */
export default function App() {
  // 订阅界面语言：换语言时这里重渲染，整棵界面跟着换文案，不用重启。
  const [language] = useLanguage();
  const [mode, setMode] = useState<WorkspaceMode>("inbox");
  const [proxiesVersion, setProxiesVersion] = useState(0);
  // 通讯录发起的写信请求；收件箱消费掉就清空，免得再次切回又弹一次。
  const [composeSeed, setComposeSeed] = useState<InboxComposeSeed>();
  // 首次启动向导要用的设置；null 表示不显示向导。
  const [firstRunSettings, setFirstRunSettings] = useState<AppSettings | null>(null);

  // 启动时读一次设置：全新安装的第一次就弹向导。读失败只记一笔，不拦启动。
  useEffect(() => {
    let cancelled = false;
    api
      .getAppSettings()
      .then((settings) => {
        if (!cancelled && settings.firstRun) setFirstRunSettings(settings);
      })
      .catch((error: unknown) => {
        console.warn(t("读取启动设置失败：{0}", [describeError(error)]));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // 让 <html lang> 跟着界面语言走，读屏和字体回退都靠它。
  useEffect(() => {
    document.documentElement.lang = language === "zh" ? "zh-CN" : "en";
  }, [language]);

  const handleComposeToContact = useCallback((contact: Contact) => {
    setComposeSeed({ to: [{ name: contact.name, address: contact.email }] });
    setMode("inbox");
  }, []);

  const handleComposeSeedConsumed = useCallback(() => setComposeSeed(undefined), []);

  return (
    <>
      <WorkspaceShell
        mode={mode}
        onModeChange={setMode}
        inbox={
          <InboxPanel
            composeSeed={composeSeed}
            onComposeSeedConsumed={handleComposeSeedConsumed}
          />
        }
        contacts={<ContactsWorkspace onCompose={handleComposeToContact} />}
        settings={
          <SettingsWorkspace
            proxiesVersion={proxiesVersion}
            onProxiesChanged={() => setProxiesVersion((value) => value + 1)}
            onGoInbox={() => setMode("inbox")}
          />
        }
      />
      {firstRunSettings !== null && (
        <FirstRunWizard settings={firstRunSettings} onDone={() => setFirstRunSettings(null)} />
      )}
    </>
  );
}

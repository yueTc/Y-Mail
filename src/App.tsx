import { useCallback, useEffect, useState } from "react";

import { api, describeError, type AppSettings, type Contact } from "./api";
import ContactsWorkspace from "./ContactsWorkspace";
import FirstRunWizard from "./FirstRunWizard";
import InboxPanel, { type InboxComposeSeed } from "./InboxPanel";
import SettingsWorkspace from "./SettingsWorkspace";
import WorkspaceShell, { type WorkspaceMode } from "./WorkspaceShell";
import { t, useLanguage } from "./i18n";
import { markVersionNotified, shouldNotifyVersion } from "./updateNotice";

/** 自动检测更新：启动后先等这么久再查第一次（毫秒）。 */
const UPDATE_CHECK_FIRST_DELAY_MS = 30_000;

/**
 * 顶层只做几件事：
 * 1. 记住当前是收件箱、通讯录还是设置；
 * 2. 记住代理列表改过几次，通知账号面板重拉「指定代理」可选项；
 * 3. 把通讯录点「写邮件」的结果转成收件箱的一次写信请求；
 * 4. 第一次启动时弹「邮件数据放哪」向导；
 * 5. 按设置定时自动检测更新，查到新版本时在设置页左栏点小红点；
 * 6. 把几块内容交给工作区外壳，切换时三边状态都保留。
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
  // 自动检测更新：默认关、间隔 24 小时；真实值启动时从设置文件读。
  const [autoCheckUpdate, setAutoCheckUpdate] = useState(false);
  const [updateCheckIntervalHours, setUpdateCheckIntervalHours] = useState(24);
  // 待提醒的新版本号；null 表示当前没有要点亮红点的版本。
  const [availableUpdateVersion, setAvailableUpdateVersion] = useState<string | null>(null);

  // 启动时读一次设置：全新安装的第一次就弹向导，同时拿自动检测配置。
  // 读失败只记一笔，不拦启动。
  useEffect(() => {
    let cancelled = false;
    api
      .getAppSettings()
      .then((settings) => {
        if (cancelled) return;
        if (settings.firstRun) setFirstRunSettings(settings);
        setAutoCheckUpdate(settings.autoCheckUpdate);
        setUpdateCheckIntervalHours(settings.updateCheckIntervalHours);
      })
      .catch((error: unknown) => {
        console.warn(t("读取启动设置失败：{0}", [describeError(error)]));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  /** 查到新版本时调这里：跟已提醒过的不一样才点亮红点，同一个版本只提醒一次。 */
  const registerAvailableVersion = useCallback((version: string) => {
    if (!version) return;
    if (!shouldNotifyVersion(version)) return;
    setAvailableUpdateVersion(version);
  }, []);

  // 自动检测调度：开关打开才排；启动满 30 秒查一次，之后按小时数循环。
  // 开关或间隔一变就清掉旧定时器，按新值重排；检查失败静默跳过。
  useEffect(() => {
    if (!autoCheckUpdate) return;
    let cancelled = false;
    let timer = 0;
    const intervalMs = Math.max(1, updateCheckIntervalHours) * 60 * 60 * 1000;
    const runOnce = async () => {
      try {
        const info = await api.checkForUpdate();
        if (!cancelled && info) registerAvailableVersion(info.version);
      } catch {
        // 自动检查失败不给用户可见报错，等下一轮再看。
      }
    };
    const schedule = (delayMs: number) => {
      timer = window.setTimeout(() => {
        void runOnce().finally(() => {
          if (!cancelled) schedule(intervalMs);
        });
      }, delayMs);
    };
    schedule(UPDATE_CHECK_FIRST_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [autoCheckUpdate, updateCheckIntervalHours, registerAvailableVersion]);

  /** 用户点开「关于」：记住当前提醒的版本，红点消失。 */
  const markUpdateNotified = useCallback(() => {
    if (availableUpdateVersion) markVersionNotified(availableUpdateVersion);
    setAvailableUpdateVersion(null);
  }, [availableUpdateVersion]);

  /** 「关于」里改了自动检测开关或间隔：立刻重排定时器，不用重启。 */
  const handleUpdateSettingsChanged = useCallback((enabled: boolean, hours: number) => {
    setAutoCheckUpdate(enabled);
    setUpdateCheckIntervalHours(hours);
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
            hasUpdateNotice={availableUpdateVersion !== null}
            onAboutOpened={markUpdateNotified}
            onUpdateFound={registerAvailableVersion}
            onUpdateSettingsChanged={handleUpdateSettingsChanged}
          />
        }
      />
      {firstRunSettings !== null && (
        <FirstRunWizard settings={firstRunSettings} onDone={() => setFirstRunSettings(null)} />
      )}
    </>
  );
}

import { useState } from "react";

import type { GitHubLoginView } from "./api";
import { safeAvatarDataUrl, safeAvatarUrl } from "./GitHubLoginPanel";
import { t } from "./i18n";

type AccountRailButtonProps = {
  profile: GitHubLoginView | null;
  onClick: () => void;
};

/** 邮箱栏底部账号入口；头像沿用登录面板的缓存优先和固定域名规则。 */
export default function AccountRailButton({ profile, onClick }: AccountRailButtonProps) {
  const [failedAvatar, setFailedAvatar] = useState<string>();
  const avatar = safeAvatarDataUrl(profile?.avatarDataUrl) ?? safeAvatarUrl(profile?.avatarUrl);
  const displayName = profile?.name?.trim() || profile?.login;

  return (
    <button
      type="button"
      className="mailbox-account-button"
      aria-label={t("GitHub 账号")}
      title={displayName ? t("{0}，打开同步", [displayName]) : t("未登录，打开同步")}
      onClick={onClick}
    >
      {avatar && avatar !== failedAvatar ? (
        <img
          key={avatar}
          className="mailbox-avatar"
          src={avatar}
          alt={t("GitHub 头像")}
          width={30}
          height={30}
          referrerPolicy="no-referrer"
          onError={() => setFailedAvatar(avatar)}
        />
      ) : (
        <svg
          className="mailbox-account-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <circle cx="12" cy="8" r="4" />
          <path d="M5 21v-2a7 7 0 0 1 14 0v2" />
        </svg>
      )}
    </button>
  );
}

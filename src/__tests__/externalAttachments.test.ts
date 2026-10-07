//! 外部大附件识别回归：网易超大附件、去重、无关链接与非 http 地址。

import { describe, expect, it } from "vitest";

import {
  externalExpiryTime,
  findExternalAttachments,
  isExternalAttachmentHref,
  isExternalExpired,
  type ExternalAttachment,
} from "../externalAttachments";

/** 真实邮件里的形状：文件名一处，下载按钮一处，指向同一个地址。 */
const NETEASE_HTML = [
  '<div style="padding:4px">',
  '<div><b>从网易163邮箱发来的超大附件</b>',
  '<a class="bigattach-mailmaster-download" href="http://u.163.com/RcUwvU56g?spm=pos.free_webmail_1.composePage.0.0.bigattachMasterDownLoad">推荐客户端极速下载</a></div>',
  '<div><div><div style="float:left;width:36px">',
  '<a href="https://mail.163.com/large-attachment-download/index.html?file=djAyZG1meTh2"></a></div>',
  '<div><div>',
  '<a href="https://mail.163.com/large-attachment-download/index.html?file=djAyZG1meTh2">虚拟.wav</a>',
  '<span style="color:#bbb"> (80.96M, 2026年8月17日 0:49 到期)</span></div>',
  '<div><a href="https://mail.163.com/large-attachment-download/index.html?file=djAyZG1meTh2">下载</a></div>',
  "</div></div></div></div>",
].join("");

/** 造一条只带到期时间的外部大附件。 */
function item(expiresText: string): ExternalAttachment {
  return {
    href: "https://mail.163.com/large-attachment-download/index.html?file=abc",
    name: "虚拟.wav",
    sizeText: "80.96M",
    expiresText,
  };
}

describe("外部大附件识别", () => {
  it("认出文件名、大小和过期时间，同一地址只留一条", () => {
    const found = findExternalAttachments(NETEASE_HTML);
    expect(found).toHaveLength(1);
    expect(found[0].name).toBe("虚拟.wav");
    expect(found[0].sizeText).toBe("80.96M");
    expect(found[0].expiresText).toBe("2026年8月17日 0:49 到期");
    expect(found[0].href).toContain("large-attachment-download");
  });

  it("链接文字是「下载」时，退回容器文字里取文件名", () => {
    const html =
      '<div><div><a href="https://mail.163.com/large-attachment-download/index.html?file=abc">下载</a>' +
      "<span> (12.5M, 2026年9月1日 0:00 到期)</span></div></div>";
    const found = findExternalAttachments(html);
    expect(found).toHaveLength(1);
    expect(found[0].name).toBe("外部大附件");
    expect(found[0].sizeText).toBe("12.5M");
  });

  it("无关链接、脚本地址和空正文都返回空", () => {
    expect(findExternalAttachments("")).toEqual([]);
    expect(findExternalAttachments('<a href="https://example.com/a.pdf">报告.pdf</a>')).toEqual([]);
    expect(
      findExternalAttachments(
        '<a href="javascript:alert(1)">large-attachment-download</a>',
      ),
    ).toEqual([]);
  });

  it("只认 http 与 https", () => {
    expect(isExternalAttachmentHref("https://mail.163.com/large-attachment-download/a")).toBe(true);
    expect(isExternalAttachmentHref("http://u.163.com/bigattach")).toBe(false);
    expect(isExternalAttachmentHref("ftp://mail.163.com/large-attachment-download/a")).toBe(false);
    expect(isExternalAttachmentHref("https://example.com/large-attachment-download")).toBe(true);
    expect(isExternalAttachmentHref("https://example.com/plain")).toBe(false);
  });

  it("到期时间按正文里写的算；时分缺省按当天 23:59", () => {
    const withTime = externalExpiryTime(item("2026年8月17日 0:49 到期"));
    expect(withTime?.getFullYear()).toBe(2026);
    expect(withTime?.getMonth()).toBe(7);
    expect(withTime?.getDate()).toBe(17);
    expect(withTime?.getHours()).toBe(0);
    expect(withTime?.getMinutes()).toBe(49);

    const dateOnly = externalExpiryTime(item("2026年8月17日 到期"));
    expect(dateOnly?.getHours()).toBe(23);
    expect(dateOnly?.getMinutes()).toBe(59);

    expect(externalExpiryTime(item("很久以前"))).toBeNull();
    expect(externalExpiryTime(item(""))).toBeNull();
  });

  it("按当前时间判断过没过期", () => {
    const expired = item("2026年8月17日 0:49 到期");
    expect(isExternalExpired(expired, new Date(2026, 9, 6))).toBe(true);
    expect(isExternalExpired(expired, new Date(2026, 7, 1))).toBe(false);
    // 认不出到期时间就不敢说过期。
    expect(isExternalExpired(item(""), new Date(2030, 0, 1))).toBe(false);
  });

  it("大小说明形状不对时不硬认", () => {
    const html =
      '<div><a href="https://mail.163.com/large-attachment-download/index.html?file=abc">虚拟.wav</a>' +
      "<span> (不是大小, 2026年9月1日 0:00 到期)</span></div>";
    const found = findExternalAttachments(html);
    expect(found).toHaveLength(1);
    expect(found[0].sizeText).toBe("");
    expect(found[0].expiresText).toBe("2026年9月1日 0:00 到期");
  });
});
# Windows Precision Touchpad 調査メモ

Issue #140 の調査結果と実装方針を記録する。

## 現状の入力経路

現在の winit 0.30.13 の Windows backend は、通常のタッチパッドスクロールを
`WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL` として受け取り、`WindowEvent::MouseWheel` の
`LineDelta` に変換する。この経路では、次の情報が失われる。

- 物理接触の開始・変更・終了
- タッチパッド由来のピクセル単位の移動量
- 物理接触と OS 慣性の境界

このアプリの `PagerInputRouter` は、ページングの対象になる精密入力について
`Began` を必要とする。したがって Windows の `LineDelta` は通常のホイール用の
フォールバックにはなるが、macOS のトラックパッドと同じページスワイプにはならない。

## Windows API の候補

Windows の Precision Touchpad は、既定ではアプリへマウスホイールとして配信される。
アプリが `RegisterTouchpadCapableWindow` を有効にすると、Windows 11 のデスクトップで
タッチパッド由来の `WM_POINTERDOWN` / `WM_POINTERUPDATE` / `WM_POINTERUP` を受け取れる。
Microsoft の推奨経路は、この pointer frame を Interaction Context に渡して
manipulation の translation を受け取る方法である。

- [Precision touchpad portal](https://learn.microsoft.com/en-us/windows/win32/input-precisiontouchpad/precision-touchpad-portal)
- [RegisterTouchpadCapableWindow](https://learn.microsoft.com/en-us/windows/win32/input-precisiontouchpad/registertouchpadcapable)
- [GetPointerTouchpadInfo / frame history](https://learn.microsoft.com/en-us/windows/win32/input-precisiontouchpad/getpointertouchpadinfo)
- [ProcessPointerFramesInteractionContext2](https://learn.microsoft.com/en-us/windows/win32/input-precisiontouchpad/processpointerframesinteractioncontext2)
- [Interaction Context](https://learn.microsoft.com/en-us/windows/win32/api/interactioncontext/)

現行の `windows` crate 0.62 には登録 API が公開されておらず、Microsoft の資料でも
user32 ordinal が示されているため、実装では user32 の ordinal 2689 を実行時解決する。
解決または登録に失敗した場合はプロセスを止めず、既存の winit wheel fallback に戻る。

## 今回の実装

`src/platform/windows/touchpad.rs` に以下を実装した。

1. ウィンドウ作成後に `RegisterTouchpadCapableWindow(hwnd, TRUE)` を呼ぶ。
2. Interaction Context を作り、translation X/Y と manipulation を有効にする。
3. `GetPointerFrameInfo` で現在の pointer frame 全体を取得し、
   `ProcessPointerFramesInteractionContext` に渡す。
4. callback の translation を既存の `RawScrollEvent` に変換する。
5. `Began/Changed/Ended/Cancelled` を `ScrollSampleAdapter` に渡し、macOS と同じ
   `Scroller` / `PagerInputRouter` / folder pager の物理を共有する。
6. Interaction Context の慣性出力は有効化しない。アプリ内の pager が contact 終了後の
   spring を担当するため、OS 慣性を重ねると二重加速になる。
7. gesture の開始時に launchpad が入力を所有していた場合は page sample として消費する。
   透明領域で始まった入力は winit が `WM_POINTER*` を `WindowEvent::Touch` として
   消費してしまうため、高精度の縦 translation を既存 passthrough と同じ
   `WM_MOUSEWHEEL` へ変換して下位ウィンドウへ配送する。

`SetPropertyInteractionContext` は画面ピクセル単位に設定しているため、translation は
物理ピクセルとして `RawScrollEvent` に渡す。DPI scale factor は App 側で更新し、既存の
軸ロック・ページ判定へそのまま渡す。

## 制約と今後の確認

- Microsoft の最新ガイドが要求する `GetPointerFrameTouchpadInfoHistory` と
  `ProcessPointerFramesInteractionContext2` は現在の crate に揃っていないため、user32 / ninput
  の ordinal を実行時解決して使っている。API が存在しない環境では登録自体を行わず、
  winit の既存 fallback に戻る。
- `RegisterTouchpadCapableWindow` は Windows 11 desktop 向け API である。Windows 10 や
  非対応ドライバーでは既存 fallback が使われる。
- 実機確認では、Windows のタッチパッド設定で自然なスクロールを切り替え、指の移動方向と
  ページ移動方向、ページ境界の rubber-band、終了後の spring、folder pager、透明領域の
  下位ウィンドウへの wheel passthrough を確認する。
- この開発環境は macOS のため、Windows 実機での pointer packet と符号は未確認である。
  Windows 側の QA では raw translation と canonical delta をログへ出す診断を追加してから
  最終調整するのが安全である。

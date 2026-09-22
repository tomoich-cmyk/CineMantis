import { useEffect, useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { getSetting, setSetting } from "@/api/settings";

const KEY = "nas_movie_root";
/** バックエンドの DEFAULT_NAS_ROOT と揃えること */
const DEFAULT_ROOT = "\\\\TYNAS\\home\\Movie";

export function NasRootSetting() {
  const qc = useQueryClient();

  const { data: saved } = useQuery({
    queryKey: ["settings", KEY],
    queryFn: () => getSetting(KEY),
  });

  const [value, setValue] = useState("");
  const [touched, setTouched] = useState(false);

  // 保存済みの値が届いたら初期表示に反映する（編集中は上書きしない）
  useEffect(() => {
    if (!touched && saved !== undefined) setValue(saved ?? "");
  }, [saved, touched]);

  const { mutate: save, isPending, isSuccess } = useMutation({
    mutationFn: (v: string) => setSetting(KEY, v),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["settings", KEY] });
      qc.invalidateQueries({ queryKey: ["nasSort"] });
      setTouched(false);
    },
  });

  const effective = (saved ?? "").trim() || DEFAULT_ROOT;

  return (
    <section className="flex flex-col gap-2 border-t border-subtle pt-6">
      <h2 className="text-sm font-medium text-gray-500">NAS 振り分け先</h2>
      <p className="text-xs text-gray-600">
        この直下に <span className="font-mono">【01】洋画</span> /{" "}
        <span className="font-mono">【02】邦画</span> があることを前提に、よみの五十音フォルダへ移動します。
      </p>
      <div className="flex gap-2">
        <input
          type="text"
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setTouched(true);
          }}
          placeholder={DEFAULT_ROOT}
          spellCheck={false}
          className="h-8 flex-1 max-w-lg bg-[#101014] border border-[#24242a] px-2 font-mono text-xs text-gray-200 placeholder-gray-700 outline-none focus:border-mantis-500"
        />
        <button
          onClick={() => save(value.trim())}
          disabled={isPending || !touched}
          className="h-8 px-4 border border-[#24242a] text-sm text-gray-400 hover:text-gray-100 hover:bg-[#121212] disabled:opacity-40 disabled:hover:text-gray-400 disabled:hover:bg-transparent"
        >
          {isPending ? "保存中…" : "保存"}
        </button>
      </div>
      <p className="text-xs text-gray-700">
        現在の適用値: <span className="font-mono">{effective}</span>
        {!(saved ?? "").trim() && "（未設定のため既定値）"}
        {isSuccess && !touched && <span className="ml-2 text-mantis-400">保存しました</span>}
      </p>
    </section>
  );
}

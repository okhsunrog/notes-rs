import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/select";
import { useHandwritingPreference } from "./input-capabilities";

const modes = [
  { value: "auto", label: "When a pen is detected" },
  { value: "always", label: "Always show" },
  { value: "hidden", label: "Hide" },
] as const;

export function HandwritingPreferenceField() {
  const { mode, setMode, mouseEnabled, setMouseEnabled } = useHandwritingPreference();
  return (
    <section className="space-y-3">
      <div>
        <h2 className="text-base font-semibold">Handwriting</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Show Write by hand in the New note menu. Applies immediately on this device.
        </p>
      </div>
      <Select
        value={mode}
        onValueChange={(value) => {
          if (value) setMode(value);
        }}
      >
        <SelectTrigger aria-label="Handwriting availability">
          {modes.find((item) => item.value === mode)?.label}
        </SelectTrigger>
        <SelectContent>
          {modes.map((item) => (
            <SelectItem key={item.value} value={item.value}>
              {item.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={mouseEnabled}
          onChange={(event) => setMouseEnabled(event.target.checked)}
        />
        Allow mouse drawing
      </label>
    </section>
  );
}

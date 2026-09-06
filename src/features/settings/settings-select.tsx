import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export function SettingsSelect<Value extends string | number>({
  label,
  value,
  options,
  disabled,
  placeholder = "Choose an option",
  onValueChange,
}: {
  label: string;
  value: Value | null;
  options: Array<{ value: Value; label: string }>;
  disabled?: boolean;
  placeholder?: string;
  onValueChange: (value: Value) => void;
}) {
  return (
    <Select
      value={value}
      items={options}
      disabled={disabled}
      onValueChange={(next) => {
        if (next !== null) onValueChange(next);
      }}
    >
      <SelectTrigger
        aria-label={label}
        className="w-full rounded-xl border-border/70 surface-base text-left shadow-none hover:bg-accent/40 dark:surface-base dark:hover:bg-accent/40 data-[size=default]:h-10"
      >
        <SelectValue className="min-w-0 truncate" placeholder={placeholder} />
      </SelectTrigger>
      <SelectContent className="w-[var(--anchor-width)] max-w-[calc(100vw-2rem)] rounded-xl border-border/70">
        {options.map((option) => (
          <SelectItem key={option.value} value={option.value} className="rounded-lg py-2">
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

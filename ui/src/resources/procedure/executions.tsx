import { useExecute, useInvalidate, useRead } from "@/lib/hooks";
import { useProcedure } from ".";
import { ICONS } from "@/lib/icons";
import ConfirmModalWithDisable from "@/components/confirm-modal-with-disable";
import { EXECUTION_ACTION_STATE_REQUERY_MS } from "@/lib/utils";

export function RunProcedure({ id }: { id: string }) {
  const invalidate = useInvalidate();
  const running = useRead(
    "GetProcedureActionState",
    { procedure: id },
    { refetchInterval: 5_000 },
  ).data?.running;
  const { mutateAsync: run, isPending } = useExecute("RunProcedure", {
    onSuccess: () =>
      setTimeout(
        () => invalidate(["GetProcedureActionState"]),
        EXECUTION_ACTION_STATE_REQUERY_MS,
      ),
  });
  const procedure = useProcedure(id);
  if (!procedure) return null;
  return (
    <ConfirmModalWithDisable
      confirmText={procedure.name}
      icon={<ICONS.Run size="1rem" />}
      onConfirm={() => run({ procedure: id })}
      disabled={running || isPending}
      loading={running || isPending}
    >
      {running ? "Running" : "Run Procedure"}
    </ConfirmModalWithDisable>
  );
}

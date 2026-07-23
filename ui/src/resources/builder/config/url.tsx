import { usePermissions, useRead, useWrite } from "@/lib/hooks";
import { Config } from "mogh_ui";
import { useLocalStorage } from "@mantine/hooks";
import { Types } from "komodo_client";

export default function UrlBuilderConfig({ id }: { id: string }) {
  const { canWrite } = usePermissions({ type: "Builder", id });
  const config = useRead("GetBuilder", { builder: id }).data?.config;

  const [update, setUpdate] = useLocalStorage<Partial<Types.UrlBuilderConfig>>({
    key: `url-builder-${id}-update-v1`,
    defaultValue: {},
  });
  const { mutateAsync } = useWrite("UpdateBuilder");

  if (!config) return null;

  const disabled = !canWrite;
  const params = config.params as Types.UrlBuilderConfig;

  return (
    <Config
      disabled={disabled}
      original={params}
      update={update}
      setUpdate={setUpdate}
      onSave={async () => {
        await mutateAsync({ id, config: { type: "Url", params: update } });
      }}
      groups={{
        "": [
          {
            label: "General",
            labelHidden: true,
            fields: {
              address: {
                description: "The address of the Periphery agent",
                placeholder: "wss://periphery:8120",
              },
              endpoint_id: {
                label: "Periphery Endpoint ID",
                description:
                  "The Iroh EndpointId of the Periphery agent. Used when connecting over Iroh.",
                placeholder: "endpoint-id",
              },
            },
          },
        ],
      }}
    />
  );
}

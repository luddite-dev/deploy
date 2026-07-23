import { usePermissions, useRead, useWrite } from "@/lib/hooks";
import { Config, ConfigInput, ConfigItem, ConfigList, ConfigSwitch } from "mogh_ui";
import { Group, Stack, Text } from "@mantine/core";
import { useLocalStorage } from "@mantine/hooks";
import { Types } from "komodo_client";
import ResourceLink from "@/resources/link";
import ResourceSelector from "@/resources/selector";
import { AccountSelectorConfig } from "@/components/config/account-selector";
import { extractRegistryDomain } from "@/lib/utils";
import DeploymentImageConfig from "./image";
import { MonacoEditor } from "mogh_ui";
import DeploymentNetworkSelector from "./network";
import SecretsSearch from "@/components/config/secrets-search";
import DeploymentRestartSelector from "./restart";
import { Link } from "react-router-dom";
import AddExtraArg from "@/components/config/add-extra-arg";
import { InputList } from "mogh_ui";
import { TerminationSignal, TerminationTimeout } from "./termination";
import { ReactNode } from "react";
import { useFullDeployment } from "..";

// The backend stores ports as Vec<PortMapping> and volumes as Vec<VolumeMount>,
// but the Monaco editor works with text. These helpers convert between the two.
// PortMapping = { container: u16, host?: u16 } — text format: "host:container"
// VolumeMount  = { volume: String, mount_path: String } — text format: "volume:/mount/path"

type PortMappingLike = Types.PortMapping;
type VolumeMountLike = Types.VolumeMount;

function portsToText(ports: unknown): string {
  if (typeof ports === "string") return ports;
  if (!Array.isArray(ports) || ports.length === 0) return "";
  return (ports as PortMappingLike[])
    .map((p) =>
      p.host != null ? `${p.host}:${p.container}` : `${p.container}`,
    )
    .join("\n");
}

function textToPorts(text: string): PortMappingLike[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith("#"))
    .map((line) => {
      const parts = line.split(":");
      if (parts.length === 2) {
        const host = parseInt(parts[0], 10);
        const container = parseInt(parts[1], 10);
        return { host, container };
      }
      return { container: parseInt(parts[0], 10) };
    })
    .filter((p) => !isNaN(p.container));
}

function volumesToText(volumes: unknown): string {
  if (typeof volumes === "string") return volumes;
  if (!Array.isArray(volumes) || volumes.length === 0) return "";
  return (volumes as VolumeMountLike[])
    .map((v) => `${v.volume}:${v.mount_path}`)
    .join("\n");
}

function textToVolumes(text: string): VolumeMountLike[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith("#"))
    .map((line) => {
      const idx = line.indexOf(":");
      if (idx === -1) return { volume: line, mount_path: "" };
      return {
        volume: line.slice(0, idx),
        mount_path: line.slice(idx + 1),
      };
    })
    .filter((v) => v.volume);
}

export default function DeploymentConfig({
  id,
  titleOther,
}: {
  id: string;
  titleOther?: ReactNode;
}) {
  const { canWrite } = usePermissions({ type: "Deployment", id });
  const config = useFullDeployment(id)?.config;
  const builds = useRead("ListBuilds", {}).data;
  const globalDisabled =
    useRead("GetCoreInfo", {}).data?.ui_write_disabled ?? false;
  const coreInfo = useRead("GetCoreInfo", {}).data;
  const ingressEnabled = coreInfo?.ingress_enabled ?? false;
  const ingressBaseDomain = coreInfo?.ingress_base_domain ?? "";
  const [update, setUpdate] = useLocalStorage<Partial<Types.DeploymentConfig>>({
    key: `deployment-${id}-update-v1`,
    defaultValue: {},
  });
  const { mutateAsync } = useWrite("UpdateDeployment");

  if (!config) return null;

  const network = update.network ?? config.network;
  const hidePorts = network === "host" || network === "none";
  const autoUpdate = update.auto_update ?? config.auto_update ?? false;

  const disabled = globalDisabled || !canWrite;

  const httpProxy = update.http_proxy ?? config.http_proxy;

  return (
    <Config
      titleOther={titleOther}
      disabled={disabled}
      original={config}
      update={update}
      setUpdate={setUpdate}
      onSave={() => mutateAsync({ id, config: update })}
      groups={{
        "": [
          {
            label: "Server",
            labelHidden: true,
            fields: {
              server_id: (server_id, set) => {
                return (
                  <ConfigItem
                    label={
                      server_id ? (
                        <Group fz="h3" fw="bold">
                          Server:
                          <ResourceLink
                            type="Server"
                            id={server_id}
                            fz="h3"
                            iconSize="1.2rem"
                          />
                        </Group>
                      ) : (
                        "Select Server"
                      )
                    }
                    description="Select the Server to deploy on."
                  >
                    <ResourceSelector
                      type="Server"
                      selected={server_id}
                      onSelect={(server_id) => set({ server_id })}
                      disabled={disabled}
                      clearable
                    />
                  </ConfigItem>
                );
              },
            },
          },
          {
            label:
              (update.image ?? config.image)?.type === "Build"
                ? "Build"
                : "Image",
            description:
              "Either pass a docker image directly, or choose a Build to deploy",
            fields: {
              image: (value, set) => (
                <DeploymentImageConfig
                  image={value}
                  setUpdate={set}
                  disabled={disabled}
                />
              ),
              image_registry_account: (account, set) => {
                const image = update.image ?? config.image;
                const provider =
                  image?.type === "Image" && image.params.image
                    ? extractRegistryDomain(image.params.image)
                    : image?.type === "Build" && image.params.build_id
                      ? builds?.find((b) => b.id === image.params.build_id)
                          ?.info.image_registry_domain
                      : undefined;
                return (
                  <AccountSelectorConfig
                    id={update.server_id ?? config.server_id ?? undefined}
                    type="Server"
                    accountType="docker"
                    provider={provider ?? "docker.io"}
                    selected={account}
                    onSelect={(image_registry_account) =>
                      set({ image_registry_account })
                    }
                    disabled={disabled}
                    placeholder={
                      image?.type === "Build" ? "Same as Build" : undefined
                    }
                    description={
                      image?.type === "Build"
                        ? "Select an alternate account used to log in to the provider"
                        : undefined
                    }
                  />
                );
              },
              redeploy_on_build: (update.image?.type ?? config.image?.type) ===
                "Build" && {
                description: "Automatically redeploy when the image is built.",
              },
            },
          },
          {
            label: "Network",
            labelHidden: true,
            fields: {
              network: (value, set) => (
                <DeploymentNetworkSelector
                  serverId={update.server_id ?? config.server_id}
                  selected={value}
                  onSelect={(network) => set({ network })}
                  disabled={disabled}
                />
              ),
              ports:
                !hidePorts &&
                ((ports, set) => (
                  <ConfigItem
                    label="Ports"
                    description="Configure port mappings."
                  >
                    <MonacoEditor
                      value={portsToText(ports) || "  # 3000:3000\n"}
                      language="key_value"
                      onValueChange={(ports) =>
                        set({ ports: textToPorts(ports) })
                      }
                      readOnly={disabled}
                    />
                  </ConfigItem>
                )),
              links: (values, set) => (
                <ConfigList
                  label="Links"
                  description="Add quick links in the resource header"
                  field="links"
                  values={values ?? []}
                  set={set}
                  disabled={disabled}
                  placeholder="Input link"
                />
              ),
            },
          },
          {
            label: "HTTP Proxy",
            description:
              "Expose this deployment through the Core ingress (Caddy reverse proxy). Requires ingress to be configured on Core.",
            hidden: !ingressEnabled,
            fields: {
              http_proxy: (_httpProxy, set) => {
                const enabled = !!httpProxy;
                return (
                  <ConfigItem
                    label="HTTP Proxy"
                    description={
                      <Group justify="space-between">
                        <Text>
                          Route external traffic to this deployment through a
                          Caddy reverse proxy.
                        </Text>
                        <ConfigSwitch
                          label={enabled ? "Enabled" : "Disabled"}
                          value={enabled}
                          onCheckedChange={(checked) =>
                            set({
                              http_proxy: checked
                                ? {
                                    subdomain: "",
                                    container_port: undefined,
                                  }
                                : undefined,
                            })
                          }
                          disabled={disabled}
                        />
                      </Group>
                    }
                  >
                    {enabled && (
                      <Stack>
                        <ConfigInput
                          label="Subdomain"
                          description={
                            ingressBaseDomain ? (
                              <Text size="sm" c="dimmed">
                                Full URL:{" "}
                                <Text inherit fw={500} component="span">
                                  https://
                                  {(update.http_proxy ?? httpProxy)?.subdomain}
                                  .{ingressBaseDomain}
                                </Text>
                              </Text>
                            ) : (
                              "The subdomain to expose this deployment on."
                            )
                          }
                          value={
                            (update.http_proxy ?? httpProxy)?.subdomain ?? ""
                          }
                          onValueChange={(value) =>
                            set({
                              http_proxy: {
                                ...(update.http_proxy ?? httpProxy)!,
                                subdomain: value,
                              },
                            })
                          }
                          disabled={disabled}
                          placeholder="my-app"
                        />
                        <ConfigInput
                          label="Container Port"
                          description="The port on the container that receives the proxied traffic. Leave empty to auto-detect."
                          value={
                            (update.http_proxy ?? httpProxy)?.container_port
                              ? String(
                                  (update.http_proxy ?? httpProxy)
                                    ?.container_port,
                                )
                              : ""
                          }
                          onValueChange={(value) => {
                            const parsed = parseInt(value, 10);
                            set({
                              http_proxy: {
                                ...(update.http_proxy ?? httpProxy)!,
                                container_port: isNaN(parsed)
                                  ? undefined
                                  : parsed,
                              },
                            });
                          }}
                          disabled={disabled}
                          placeholder="auto"
                        />
                      </Stack>
                    )}
                  </ConfigItem>
                );
              },
            },
          },
          {
            label: "Environment",
            description: "Pass these variables to the container",
            fields: {
              environment: (env, set) => (
                <Stack>
                  <SecretsSearch
                    server={update.server_id ?? config.server_id}
                  />
                  <MonacoEditor
                    value={env || "  # VARIABLE = value\n"}
                    onValueChange={(environment) => set({ environment })}
                    language="key_value"
                    readOnly={disabled}
                  />
                </Stack>
              ),
              // skip_secret_interp: true,
            },
          },
          {
            label: "Volumes",
            description: "Configure the volume bindings.",
            fields: {
              volumes: (volumes, set) => (
                <MonacoEditor
                  value={volumesToText(volumes) || "  # volume:/container/path\n"}
                  language="key_value"
                  onValueChange={(volumes) =>
                    set({ volumes: textToVolumes(volumes) })
                  }
                  readOnly={disabled}
                />
              ),
            },
          },
          {
            label: "Restart",
            labelHidden: true,
            fields: {
              restart: (value, set) => (
                <DeploymentRestartSelector
                  selected={value}
                  set={set}
                  disabled={disabled}
                />
              ),
            },
          },
          {
            label: "Auto Update",
            hidden: (update.image ?? config.image)?.type === "Build",
            fields: {
              poll_for_updates: (poll, set) => {
                return (
                  <ConfigSwitch
                    label="Poll for Updates"
                    description="Check for updates to the image during Global Auto Update."
                    value={autoUpdate || poll}
                    onCheckedChange={(poll_for_updates) =>
                      set({ poll_for_updates })
                    }
                    disabled={disabled || autoUpdate}
                  />
                );
              },
              auto_update: {
                description: "Trigger a redeploy if a newer image is found.",
              },
            },
          },
        ],
        advanced: [
          {
            label: "Command",
            labelHidden: true,
            fields: {
              command: (value, set) => (
                <ConfigItem
                  label="Command"
                  description={
                    <Group>
                      <Text>Replace the CMD, or extend the ENTRYPOINT.</Text>
                      <Link
                        to="https://docs.docker.com/engine/reference/run/#commands-and-arguments"
                        target="_blank"
                      >
                        See docker docs.
                      </Link>
                    </Group>
                  }
                >
                  <MonacoEditor
                    value={value}
                    language="shell"
                    onValueChange={(command) => set({ command })}
                    readOnly={disabled}
                  />
                </ConfigItem>
              ),
            },
          },
          {
            label: "Labels",
            description: "Attach --labels to the container.",
            fields: {
              labels: (labels, set) => (
                <MonacoEditor
                  value={labels || "  # your.docker.label: value\n"}
                  language="key_value"
                  onValueChange={(labels) => set({ labels })}
                  readOnly={disabled}
                />
              ),
            },
          },
          {
            label: "Extra Args",
            labelHidden: true,
            fields: {
              extra_args: (value, set) => (
                <ConfigItem
                  label="Extra Args"
                  description={
                    <div className="flex flex-row flex-wrap gap-2">
                      <div>
                        Pass extra arguments to 'docker run'.
                      </div>
                      <Link
                        to="https://docs.docker.com/reference/cli/docker/container/run/#options"
                        target="_blank"
                        className="text-primary"
                      >
                        See docker docs.
                      </Link>
                    </div>
                  }
                >
                  {!disabled && (
                    <AddExtraArg
                      type="Deployment"
                      onSelect={(suggestion) =>
                        set({
                          extra_args: [
                            ...(update.extra_args ?? config.extra_args ?? []),
                            suggestion,
                          ],
                        })
                      }
                      disabled={disabled}
                    />
                  )}
                  <InputList
                    field="extra_args"
                    values={value ?? []}
                    set={set}
                    disabled={disabled}
                    placeholder="--extra-arg=value"
                  />
                </ConfigItem>
              ),
            },
          },
          {
            label: "Termination",
            description:
              "Configure the signals used to 'docker stop' the container. Options are SIGTERM, SIGQUIT, SIGINT, and SIGHUP.",
            fields: {
              termination_signal: (value, set) => (
                <TerminationSignal arg={value} set={set} disabled={disabled} />
              ),
              termination_timeout: (value, set) => (
                <TerminationTimeout arg={value} set={set} disabled={disabled} />
              ),
              term_signal_labels: (value, set) => (
                <ConfigItem
                  label="Termination Signal Labels"
                  description="Choose between multiple signals when stopping"
                >
                  <MonacoEditor
                    value={value || DEFAULT_TERM_SIGNAL_LABELS}
                    language="key_value"
                    onValueChange={(term_signal_labels) =>
                      set({ term_signal_labels })
                    }
                    readOnly={disabled}
                  />
                </ConfigItem>
              ),
            },
          },
        ],
      }}
    />
  );
}

const DEFAULT_TERM_SIGNAL_LABELS = `  # SIGTERM: sigterm label
  # SIGQUIT: sigquit label
  # SIGINT: sigint label
  # SIGHUP: sighup label
`;

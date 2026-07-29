#pragma once

#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

#include <fcitx-config/configuration.h>
#include <fcitx-config/iniparser.h>
#include <fcitx-config/option.h>
#include <fcitx-config/rawconfig.h>
#include <fcitx-utils/dbus/bus.h>
#include <fcitx-utils/handlertable.h>
#include <fcitx-utils/key.h>
#include <fcitx-utils/trackableobject.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addoninstance.h>
#include <fcitx/inputcontext.h>
#include <fcitx/inputcontextproperty.h>
#include <fcitx/instance.h>

namespace natsu_typeless {

struct AsrProviderAnnotation : public fcitx::EnumAnnotation {
    void dumpDescription(fcitx::RawConfig &config) const {
        fcitx::EnumAnnotation::dumpDescription(config);
        config.setValueByPath("Enum/0", "local");
        config.setValueByPath("EnumI18n/0", "Local Qwen3-ASR");
        config.setValueByPath("Enum/1", "qwen_api");
        config.setValueByPath("EnumI18n/1", "Qwen ASR API");
        config.setValueByPath("Enum/2", "openai_compatible");
        config.setValueByPath("EnumI18n/2",
                              "OpenAI-compatible transcription API");
    }
};

FCITX_CONFIGURATION(
    NatsuTypelessConfig,
    fcitx::KeyListOption triggerKey{
        this,
        "TriggerKey",
        "Hold-to-talk key",
        {fcitx::Key("Alt_R")},
        fcitx::KeyListConstrain(
            {fcitx::KeyConstrainFlag::AllowModifierLess,
             fcitx::KeyConstrainFlag::AllowModifierOnly})};
    fcitx::Option<std::string> language{
        this, "Language", "Recognition language (auto, zh, en)", "auto"};
    fcitx::Option<std::vector<std::string>> vocabulary{
        this, "Vocabulary",
        "Preferred terms or heard-form => canonical-form corrections", {}};
    fcitx::OptionWithAnnotation<std::string, AsrProviderAnnotation> asrProvider{
        this, "AsrProvider", "ASR provider", "local"};
    fcitx::Option<std::string> asrApiBase{
        this, "AsrApiBase", "ASR API base URL",
        "https://dashscope.aliyuncs.com/compatible-mode/v1"};
    fcitx::Option<std::string> asrModel{
        this, "AsrModel", "ASR API model ID", "qwen3-asr-flash"};
    fcitx::Option<bool> asrApiKeyRequired{
        this, "AsrApiKeyRequired", "Require Bearer API key for ASR", true};
    fcitx::Option<int> asrTimeoutMs{
        this, "AsrTimeoutMs", "ASR API timeout in milliseconds", 30000};
    fcitx::Option<bool> cloudEnabled{
        this, "CloudPostprocess", "Use cloud text post-processing", true};
    fcitx::Option<std::string> cloudApiBase{
        this, "CloudApiBase", "OpenAI-compatible API base URL",
        "https://generativelanguage.googleapis.com/v1beta/openai"};
    fcitx::Option<std::string> cloudModel{
        this, "CloudModel", "OpenAI-compatible model ID",
        "gemini-3.5-flash-lite"};
    fcitx::Option<bool> cloudApiKeyRequired{
        this, "CloudApiKeyRequired", "Require Bearer API key", true};
    fcitx::Option<bool> surroundingContext{
        this, "SurroundingContext",
        "Send bounded surrounding text to the cloud model", false};
    fcitx::Option<int> cloudTimeoutMs{
        this, "CloudTimeoutMs", "Cloud timeout in milliseconds", 4000};
    fcitx::Option<int> modelIdleMinutes{
        this, "ModelIdleMinutes", "Unload the local ASR model after idle minutes", 15};
    fcitx::Option<int> maxRecordingSeconds{
        this, "MaxRecordingSeconds", "Maximum recording duration", 120};);

enum class SessionPhase {
    Idle,
    Starting,
    Recording,
    Transcribing,
    Polishing,
    ResultReady,
    TakingResult,
};

class SessionState final : public fcitx::InputContextProperty {
public:
    SessionPhase phase = SessionPhase::Idle;
    std::string id;
    std::unique_ptr<fcitx::dbus::Slot> pendingCall;
};

class NatsuTypeless final : public fcitx::AddonInstance {
public:
    explicit NatsuTypeless(fcitx::Instance *instance);
    ~NatsuTypeless() override;

    const fcitx::Configuration *getConfig() const override { return &config_; }
    void setConfig(const fcitx::RawConfig &config) override;
    void reloadConfig() override;

private:
    void handleKey(fcitx::KeyEvent &event);
    bool canStart(fcitx::InputContext *inputContext) const;
    void begin(fcitx::InputContext *inputContext, fcitx::KeyEvent &event);
    void end(fcitx::InputContext *inputContext, fcitx::KeyEvent &event);
    void cancel(fcitx::InputContext *inputContext, bool notifyDaemon);
    void clearSession(fcitx::InputContext *inputContext);
    void updateStatus(fcitx::InputContext *inputContext,
                      const std::string &text);
    void configureDaemon();
    void handleStateSignal(fcitx::dbus::Message &message);
    void handleResultSignal(fcitx::dbus::Message &message);
    void takeResult(const std::string &sessionId);
    void showError(fcitx::InputContext *inputContext,
                   const std::string &message);

    fcitx::Instance *instance_;
    fcitx::AddonInstance *dbusAddon_ = nullptr;
    fcitx::dbus::Bus *bus_ = nullptr;
    NatsuTypelessConfig config_;
    fcitx::FactoryFor<SessionState> stateFactory_;
    std::vector<std::unique_ptr<fcitx::HandlerTableEntryBase>> eventHandlers_;
    std::vector<std::unique_ptr<fcitx::dbus::Slot>> signalSlots_;
    std::unique_ptr<fcitx::dbus::Slot> configureCall_;
    std::unordered_map<
        std::string, fcitx::TrackableObjectReference<fcitx::InputContext>>
        sessions_;
};

class NatsuTypelessFactory final : public fcitx::AddonFactory {
public:
    fcitx::AddonInstance *create(fcitx::AddonManager *manager) override;
};

} // namespace natsu_typeless
